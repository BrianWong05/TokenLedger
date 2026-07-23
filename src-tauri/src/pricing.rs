use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use ts_rs::TS;

/// Providers whose normalized entry wins a collision over prefixed resellers.
const CANONICAL: &[&str] = &["anthropic", "openai", "gemini", "vertex_ai-language-models"];

const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// lowercase -> strip through last '/' -> strip a trailing `-YYYYMMDD` suffix.
pub fn normalize_model(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let after_slash = match lower.rfind('/') {
        Some(i) => &lower[i + 1..],
        None => &lower[..],
    };
    // Inspect bytes so a multibyte char near the end can never make split panic:
    // only truncate when the last 9 bytes are exactly `-` + 8 ASCII digits, which
    // guarantees len-9 is a char boundary (the tail is pure ASCII).
    let b = after_slash.as_bytes();
    if b.len() >= 9 {
        let tail = &b[b.len() - 9..];
        if tail[0] == b'-' && tail[1..].iter().all(|c| c.is_ascii_digit()) {
            return after_slash[..after_slash.len() - 9].to_string();
        }
    }
    after_slash.to_string()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

impl Rates {
    /// List-price value of a token bundle at these rates. The single home of
    /// the pricing formula — every query (summary/trend/series/breakdown)
    /// must call this so a rate change can never make panels disagree.
    pub fn cost(&self, input: i64, output: i64, cache_read: i64, w5: i64, w1: i64) -> f64 {
        input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * self.cache_read
            + w5 as f64 * self.cache_write_5m
            + w1 as f64 * self.cache_write_1h
    }

    /// Cache tokens were used but their rate is missing → the model is
    /// Cache-Estimated (CONTEXT.md).
    /// ponytail: prices store an absent cache rate as 0.0, so "no rate" == 0.0
    /// here; distinguish-explicit-zero needs nullable price columns — add if a
    /// catalog ever prices cache at $0.
    pub fn cache_gap(&self, cache_read: i64, w5: i64, w1: i64) -> bool {
        (cache_read > 0 && self.cache_read == 0.0)
            || (w5 > 0 && self.cache_write_5m == 0.0)
            || (w1 > 0 && self.cache_write_1h == 0.0)
    }

    /// Project onto the frontend's per-token shape. A 0.0 rate maps to None so
    /// the Pricing tab can render "no rate" (the Cache-Estimated signal) — same
    /// "absent == 0.0" convention prices already store under (see cache_gap).
    /// The single cache_write is the 5m/base rate (1h mirrors it at write time).
    fn to_per_tok(self) -> RatesPerTok {
        fn opt(v: f64) -> Option<f64> {
            (v != 0.0).then_some(v)
        }
        RatesPerTok {
            input: opt(self.input),
            output: opt(self.output),
            cache_read: opt(self.cache_read),
            cache_write: opt(self.cache_write_5m),
        }
    }
}

/// A candidate price row with Option fields so merges can honor
/// "never overwrite a non-null value with a null one".
#[derive(Clone)]
struct Row {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cw5m: Option<f64>,
    cw1h: Option<f64>,
}

fn cost(entry: &serde_json::Value, key: &str) -> Option<f64> {
    // as_f64 returns None for null AND for string placeholders (e.g. sample_spec).
    entry.get(key).and_then(|v| v.as_f64())
}

fn write_price_row(conn: &Connection, model: &str, row: &Row) -> rusqlite::Result<()> {
    // 1h TTL falls back to the 5m rate when absent; null -> 0 at write time.
    let cw5m = row.cw5m.unwrap_or(0.0);
    let cw1h = row.cw1h.or(row.cw5m).unwrap_or(0.0);
    conn.execute(
        "INSERT OR REPLACE INTO prices \
         (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            model,
            row.input.unwrap_or(0.0),
            row.output.unwrap_or(0.0),
            row.cache_read.unwrap_or(0.0),
            cw5m,
            cw1h,
        ],
    )?;
    Ok(())
}

/// Rebuild the `prices` table from a LiteLLM JSON snapshot. Writes an exact row
/// (model = the raw LiteLLM key) for every entry with a non-null input OR output
/// cost, plus guarded normalized fallback rows. Returns the row count.
pub fn rebuild_prices(conn: &mut Connection, litellm_json: &str) -> Result<u64, String> {
    let root: serde_json::Value =
        serde_json::from_str(litellm_json).map_err(|e| format!("parse litellm json: {e}"))?;
    let obj = root
        .as_object()
        .ok_or_else(|| "litellm json is not an object".to_string())?;

    let mut exact: Vec<(String, Row)> = Vec::new();
    let mut norm: HashMap<String, (Row, bool)> = HashMap::new(); // key -> (row, canonical)

    for (key, entry) in obj {
        let input = cost(entry, "input_cost_per_token");
        let output = cost(entry, "output_cost_per_token");
        // Skip entries whose input AND output are both null/non-numeric.
        if input.is_none() && output.is_none() {
            continue;
        }
        let row = Row {
            input,
            output,
            cache_read: cost(entry, "cache_read_input_token_cost"),
            cw5m: cost(entry, "cache_creation_input_token_cost"),
            cw1h: cost(entry, "cache_creation_input_token_cost_above_1hr"),
        };
        exact.push((key.clone(), row.clone()));

        let canonical = entry
            .get("litellm_provider")
            .and_then(|v| v.as_str())
            .map(|p| CANONICAL.contains(&p))
            .unwrap_or(false);
        let nkey = normalize_model(key);
        match norm.get_mut(&nkey) {
            None => {
                norm.insert(nkey, (row, canonical));
            }
            Some((existing, existing_canon)) => {
                let new_wins = canonical && !*existing_canon;
                if new_wins {
                    // New (canonical) row wins; keep an old non-null field only where new is null.
                    existing.input = row.input.or(existing.input);
                    existing.output = row.output.or(existing.output);
                    existing.cache_read = row.cache_read.or(existing.cache_read);
                    existing.cw5m = row.cw5m.or(existing.cw5m);
                    existing.cw1h = row.cw1h.or(existing.cw1h);
                    *existing_canon = true;
                } else {
                    // New row does not win; only fill fields the existing row lacks.
                    existing.input = existing.input.or(row.input);
                    existing.output = existing.output.or(row.output);
                    existing.cache_read = existing.cache_read.or(row.cache_read);
                    existing.cw5m = existing.cw5m.or(row.cw5m);
                    existing.cw1h = existing.cw1h.or(row.cw1h);
                }
            }
        }
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM prices", []).map_err(|e| e.to_string())?;
    // Normalized rows first; exact rows overwrite on key collision (exact is authoritative).
    for (model, (row, _)) in &norm {
        write_price_row(&tx, model, row).map_err(|e| e.to_string())?;
    }
    for (model, row) in &exact {
        write_price_row(&tx, model, row).map_err(|e| e.to_string())?;
    }
    let count: u64 = tx
        .query_row("SELECT COUNT(*) FROM prices", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(count)
}

/// Fetch the latest LiteLLM snapshot (10s timeout) and return its JSON. Does NO DB
/// work so callers can run the blocking network fetch outside the DB lock. On fetch
/// failure falls back to the cached file, then the bundled snapshot.
pub fn load_prices_json(cache_dir: &Path) -> String {
    let cache_file = cache_dir.join("model_prices.json");
    let fetched = ureq::get(LITELLM_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|resp| resp.into_string().ok());
    if let Some(body) = fetched {
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(&cache_file, &body);
        return body;
    }
    if let Ok(body) = std::fs::read_to_string(&cache_file) {
        return body;
    }
    include_str!("../resources/model_prices.json").to_string()
}

/// Fetch the latest LiteLLM snapshot and rebuild the prices table.
/// Production splits these two steps (fetch outside the DB lock); this convenience
/// wrapper is retained for the e2e test, hence test-only in non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String> {
    let json = load_prices_json(cache_dir);
    rebuild_prices(conn, &json)
}

#[derive(Debug, Clone, Copy)]
pub struct OverrideRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// Per-token USD rates as the frontend edits/displays them: nullable fields, a
/// single cache_write (applied to both TTLs at write time). Structurally the
/// same as OverrideRates; kept distinct because it is the IPC contract the
/// Pricing tab consumes (override_rates, catalog rates, and set_model_override).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct RatesPerTok {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl From<RatesPerTok> for OverrideRates {
    fn from(r: RatesPerTok) -> Self {
        OverrideRates {
            input: r.input,
            output: r.output,
            cache_read: r.cache_read,
            cache_write: r.cache_write,
        }
    }
}

/// A catalog List Price match: which catalog it came from (ADR-0003) and its
/// rates. `origin` is "litellm" | "openrouter"; v1 only reads LiteLLM.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CatalogRates {
    pub origin: String,
    pub rates: RatesPerTok,
}

/// One row of the Pricing tab: a Model seen in the Ledger, the Source it came
/// from, its raw Override (if any), and its best catalog match resolved WITHOUT
/// the Override. The frontend derives Unpriced/Cache-Estimated/override states
/// from this shape (no state enum): Unpriced = neither field set; Cache-Estimated
/// = catalog priced for input/output but cache rates null.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ModelPricing {
    pub model: String,
    pub tool: String,
    pub override_rates: Option<RatesPerTok>,
    pub catalog: Option<CatalogRates>,
}

pub fn set_override(conn: &Connection, model: &str, rates: OverrideRates) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO price_overrides \
         (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_per_tok) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![model, rates.input, rates.output, rates.cache_read, rates.cache_write],
    )?;
    Ok(())
}

pub fn delete_override(conn: &Connection, model: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM price_overrides WHERE model = ?1",
        rusqlite::params![model],
    )?;
    Ok(())
}

pub struct RateMap {
    prices: HashMap<String, Rates>,
    overrides: HashMap<String, Rates>,
}

impl RateMap {
    pub fn load(conn: &Connection) -> rusqlite::Result<RateMap> {
        let mut prices = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, \
             cache_write_5m_per_tok, cache_write_1h_per_tok FROM prices",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Rates {
                    input: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    output: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    cache_read: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    cache_write_5m: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                    cache_write_1h: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                },
            ))
        })?;
        for row in rows {
            let (m, rt) = row?;
            prices.insert(m, rt);
        }

        let mut overrides = HashMap::new();
        let mut stmt2 = conn.prepare(
            "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, \
             cache_write_per_tok FROM price_overrides",
        )?;
        let orows = stmt2.query_map([], |r| {
            // Override's single cache_write applies to BOTH TTLs; None -> 0.
            let cw = r.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
            Ok((
                r.get::<_, String>(0)?,
                Rates {
                    input: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    output: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    cache_read: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    cache_write_5m: cw,
                    cache_write_1h: cw,
                },
            ))
        })?;
        for row in orows {
            let (m, rt) = row?;
            overrides.insert(m, rt);
        }

        Ok(RateMap { prices, overrides })
    }

    /// override (raw name) -> exact price (raw name) -> normalized price. None = unpriced.
    pub fn resolve(&self, raw_model: &str) -> Option<Rates> {
        if let Some(r) = self.overrides.get(raw_model) {
            return Some(*r);
        }
        self.resolve_catalog(raw_model).map(|(_, r)| r)
    }

    /// The catalog tier of `resolve`, ignoring overrides: exact raw key ->
    /// normalized key, reporting which catalog matched. v1 reads only LiteLLM
    /// (ADR-0003), so a hit is always "litellm"; the OpenRouter fallback tier
    /// plugs in right here when it lands.
    /// ponytail: origin hardcoded "litellm" until the prices table carries a
    /// per-row source column — add that column with the OpenRouter tier.
    pub fn resolve_catalog(&self, raw_model: &str) -> Option<(&'static str, Rates)> {
        if let Some(r) = self.prices.get(raw_model) {
            return Some(("litellm", *r));
        }
        self.prices
            .get(&normalize_model(raw_model))
            .map(|r| ("litellm", *r))
    }
}

/// Raw Overrides straight from price_overrides (nulls preserved, unlike
/// RateMap which zero-fills), keyed by raw Model name — what the Pricing editor
/// shows/edits.
fn load_overrides_raw(conn: &Connection) -> rusqlite::Result<HashMap<String, RatesPerTok>> {
    let mut stmt = conn.prepare(
        "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_per_tok \
         FROM price_overrides",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            RatesPerTok {
                input: r.get(1)?,
                output: r.get(2)?,
                cache_read: r.get(3)?,
                cache_write: r.get(4)?,
            },
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (m, rt) = row?;
        map.insert(m, rt);
    }
    Ok(map)
}

/// Every distinct Model in the Ledger with its Source, raw Override, and best
/// catalog match (resolved without the Override). Lists Models regardless of
/// pricing state — including Unpriced ones with no tokens priced.
pub fn model_pricing(conn: &Connection) -> rusqlite::Result<Vec<ModelPricing>> {
    let rates = RateMap::load(conn)?;
    let overrides = load_overrides_raw(conn)?;

    // Order so the first row per model is the most-frequent Source (the `tool`);
    // grouping by model keeps a model's rows contiguous for the first-wins scan.
    let mut stmt = conn.prepare(
        "SELECT model, source, COUNT(*) AS c FROM events WHERE model IS NOT NULL \
         GROUP BY model, source ORDER BY model, c DESC, source ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;

    let mut out: Vec<ModelPricing> = Vec::new();
    for row in rows {
        let (model, source) = row?;
        if out.last().map(|m| m.model.as_str()) == Some(model.as_str()) {
            continue; // already recorded this model with its most-frequent Source
        }
        let catalog = rates.resolve_catalog(&model).map(|(origin, rt)| CatalogRates {
            origin: origin.to_string(),
            rates: rt.to_per_tok(),
        });
        out.push(ModelPricing {
            override_rates: overrides.get(&model).copied(),
            tool: source,
            model,
            catalog,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ~8-entry LiteLLM slice. Field names verified against the real
    // model_prices_and_context_window.json. `sample_spec` has string
    // costs (skipped via as_f64), `chatgpt/gpt-5.4` is all-null (skipped),
    // `replicate/.../gemini-2.5-flash` is a non-canonical reseller collision.
    const FIXTURE: &str = r#"{
      "sample_spec": {
        "input_cost_per_token": "float",
        "output_cost_per_token": "float",
        "litellm_provider": "example"
      },
      "gpt-5.4": {
        "input_cost_per_token": 2.5e-06,
        "output_cost_per_token": 1e-05,
        "cache_read_input_token_cost": 2.5e-07,
        "litellm_provider": "openai"
      },
      "chatgpt/gpt-5.4": {
        "input_cost_per_token": null,
        "output_cost_per_token": null,
        "litellm_provider": "openai"
      },
      "claude-sonnet-4-5": {
        "input_cost_per_token": 3e-06,
        "output_cost_per_token": 1.5e-05,
        "cache_read_input_token_cost": 3e-07,
        "cache_creation_input_token_cost": 3.75e-06,
        "cache_creation_input_token_cost_above_1hr": 6e-06,
        "litellm_provider": "anthropic"
      },
      "gemini-2.5-flash": {
        "input_cost_per_token": 3e-07,
        "output_cost_per_token": 2.5e-06,
        "cache_read_input_token_cost": 3e-08,
        "litellm_provider": "vertex_ai-language-models"
      },
      "replicate/meta/gemini-2.5-flash": {
        "input_cost_per_token": 2.5e-06,
        "output_cost_per_token": 2.5e-06,
        "litellm_provider": "replicate"
      },
      "claude-3-5-sonnet-20241022": {
        "input_cost_per_token": 3e-06,
        "output_cost_per_token": 1.5e-05,
        "cache_creation_input_token_cost": 3.75e-06,
        "litellm_provider": "anthropic"
      }
    }"#;

    fn test_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("test.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn normalize_strips_slash_and_date_suffix() {
        assert_eq!(normalize_model("GPT-5.4"), "gpt-5.4");
        assert_eq!(normalize_model("anthropic/claude-3-5-sonnet-20241022"), "claude-3-5-sonnet");
        assert_eq!(normalize_model("claude-sonnet-4-5"), "claude-sonnet-4-5"); // -4-5 is not -\d{8}
        assert_eq!(normalize_model("replicate/meta/gemini-2.5-flash"), "gemini-2.5-flash");
    }

    #[test]
    fn normalize_is_byte_safe_on_multibyte_input() {
        // len()-9 lands mid-UTF-8-char for these; the byte inspection must not
        // panic and must leave the (non-date-suffixed) name intact once lowercased.
        assert_eq!(normalize_model("café-modeléxyz"), "café-modeléxyz");
        assert_eq!(normalize_model("模型-2.5-flashé"), "模型-2.5-flashé");
        // A real -YYYYMMDD suffix (pure ASCII tail) still strips correctly.
        assert_eq!(normalize_model("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
    }

    #[test]
    fn rebuild_counts_distinct_rows() {
        let (_d, mut conn) = test_conn();
        // 5 exact rows + 4 normalized keys, unioned = 6 distinct model rows.
        let n = rebuild_prices(&mut conn, FIXTURE).unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn exact_wins_and_null_reseller_does_not_pollute() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // gpt-5.4 exact hit.
        assert_eq!(rm.resolve("gpt-5.4").unwrap().input, 2.5e-06);
        // The all-null chatgpt/gpt-5.4 was skipped, so it created no null
        // normalized row; it resolves to the canonical gpt-5.4 price.
        assert_eq!(rm.resolve("chatgpt/gpt-5.4").unwrap().input, 2.5e-06);
    }

    #[test]
    fn canonical_wins_normalized_collision() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Not an exact key -> normalized to gemini-2.5-flash; canonical 3e-07
        // must win over the 2.5e-06 reseller.
        assert_eq!(rm.resolve("gemini-2.5-flash-20250101").unwrap().input, 3e-07);
    }

    #[test]
    fn claude_cache_rates_and_1h_fallback() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        let r = rm.resolve("claude-sonnet-4-5").unwrap();
        assert_eq!(r.cache_read, 3e-07);
        assert_eq!(r.cache_write_5m, 3.75e-06);
        assert_eq!(r.cache_write_1h, 6e-06);
        // claude-3-5-sonnet-20241022 has 5m cost but no above_1hr -> 1h falls back to 5m.
        let f = rm.resolve("claude-3-5-sonnet-20241022").unwrap();
        assert_eq!(f.cache_write_5m, 3.75e-06);
        assert_eq!(f.cache_write_1h, 3.75e-06);
    }

    #[test]
    fn unknown_model_is_none() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("totally-unknown-model"), None);
    }

    #[test]
    fn override_wins_fills_none_and_applies_cache_write_both_ttls() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE).unwrap();
        set_override(
            &conn,
            "gemini-2.5-flash",
            OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: Some(1e-06) },
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        let r = rm.resolve("gemini-2.5-flash").unwrap();
        assert_eq!(r.input, 9e-06);          // override beats LiteLLM 3e-07
        assert_eq!(r.output, 0.0);           // None -> 0
        assert_eq!(r.cache_read, 0.0);       // None -> 0
        assert_eq!(r.cache_write_5m, 1e-06); // cache_write on both TTLs
        assert_eq!(r.cache_write_1h, 1e-06);
        // Delete restores the LiteLLM price.
        delete_override(&conn, "gemini-2.5-flash").unwrap();
        let rm2 = RateMap::load(&conn).unwrap();
        assert_eq!(rm2.resolve("gemini-2.5-flash").unwrap().input, 3e-07);
    }

    // Catalog with two priced models: one full (input/output/cache), one with
    // input/output only (missing cache -> Cache-Estimated signal).
    const MP_FIXTURE: &str = r#"{
      "priced-full": {
        "input_cost_per_token": 3e-06,
        "output_cost_per_token": 6e-06,
        "cache_read_input_token_cost": 3e-07,
        "cache_creation_input_token_cost": 3.75e-06,
        "litellm_provider": "anthropic"
      },
      "priced-no-cache": {
        "input_cost_per_token": 1e-06,
        "output_cost_per_token": 2e-06,
        "litellm_provider": "openai"
      }
    }"#;

    // model_pricing only reads (model, source) from events; tokens are filler and
    // default to 0. Unique dedup_key per call so repeats aren't deduped away.
    fn seed_event(conn: &Connection, model: &str, source: &str) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let k = N.fetch_add(1, Ordering::Relaxed);
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, source_file) \
             VALUES (?1, ?2, 0, ?3, 'f')",
            rusqlite::params![format!("k{k}"), source, model],
        )
        .unwrap();
    }

    #[test]
    fn model_pricing_omits_unattributed_usage() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE).unwrap();
        seed_event(&conn, "priced-full", "claude");
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, input_tokens, source_file) \
             VALUES ('pi:tool-result:1', 'pi', 0, NULL, 100, 'pi.jsonl')",
            [],
        ).unwrap();

        let list = model_pricing(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].model, "priced-full");
    }

    #[test]
    fn model_pricing_splits_override_and_catalog() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE).unwrap();
        seed_event(&conn, "priced-no-cache", "codex");
        seed_event(&conn, "priced-full", "claude");
        seed_event(&conn, "unpriced-x", "grok");
        // Multi-source model: gemini x2 beats codex x1 for the `tool` pick.
        seed_event(&conn, "multi", "gemini");
        seed_event(&conn, "multi", "gemini");
        seed_event(&conn, "multi", "codex");
        // An Override on a catalogued model: both override_rates and catalog set.
        set_override(
            &conn,
            "priced-full",
            OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: Some(1e-06) },
        )
        .unwrap();

        let list = model_pricing(&conn).unwrap();
        let get = |name: &str| list.iter().find(|m| m.model == name).unwrap();
        // Every distinct Model is listed, regardless of pricing state.
        for m in ["priced-no-cache", "priced-full", "unpriced-x", "multi"] {
            assert!(list.iter().any(|r| r.model == m), "missing {m}");
        }

        // Unpriced: neither override nor catalog.
        let u = get("unpriced-x");
        assert!(u.override_rates.is_none());
        assert!(u.catalog.is_none());
        assert_eq!(u.tool, "grok");

        // Catalog with missing cache rates: input/output Some, cache None.
        let nc = get("priced-no-cache");
        assert!(nc.override_rates.is_none());
        let cat = nc.catalog.as_ref().unwrap();
        assert_eq!(cat.origin, "litellm");
        assert_eq!(cat.rates.input, Some(1e-06));
        assert_eq!(cat.rates.output, Some(2e-06));
        assert_eq!(cat.rates.cache_read, None);
        assert_eq!(cat.rates.cache_write, None);
        assert_eq!(nc.tool, "codex");

        // Overridden model: raw Override (nulls preserved) AND catalog resolved
        // WITHOUT the override.
        let ov = get("priced-full");
        let orr = ov.override_rates.unwrap();
        assert_eq!(orr.input, Some(9e-06));
        assert_eq!(orr.output, None); // raw null kept, not zero-filled
        assert_eq!(orr.cache_write, Some(1e-06));
        let ocat = ov.catalog.as_ref().unwrap();
        assert_eq!(ocat.rates.input, Some(3e-06)); // catalog, not the 9e-06 override
        assert_eq!(ocat.rates.cache_read, Some(3e-07));

        // Most-frequent Source wins the `tool`.
        assert_eq!(get("multi").tool, "gemini");
    }

    #[test]
    fn override_set_delete_roundtrip_via_model_pricing() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE).unwrap();
        seed_event(&conn, "priced-full", "claude");

        let find = |list: &[ModelPricing]| {
            list.iter().find(|m| m.model == "priced-full").cloned().unwrap()
        };

        // No override initially; catalog present.
        let before = find(&model_pricing(&conn).unwrap());
        assert!(before.override_rates.is_none());
        assert!(before.catalog.is_some());

        // Set (the core the set_model_override command wraps).
        set_override(
            &conn,
            "priced-full",
            OverrideRates { input: Some(5e-06), output: Some(5e-06), cache_read: None, cache_write: None },
        )
        .unwrap();
        let mid = find(&model_pricing(&conn).unwrap());
        assert_eq!(mid.override_rates.unwrap().input, Some(5e-06));

        // Delete -> falls back to the catalog List Price.
        delete_override(&conn, "priced-full").unwrap();
        let after = find(&model_pricing(&conn).unwrap());
        assert!(after.override_rates.is_none());
        assert!(after.catalog.is_some());
    }
}
