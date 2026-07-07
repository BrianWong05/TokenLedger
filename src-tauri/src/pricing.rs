use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Providers whose normalized entry wins a collision over prefixed resellers.
const CANONICAL: &[&str] = &["anthropic", "openai", "gemini", "vertex_ai-language-models"];

const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// lowercase -> strip through last '/' -> strip a trailing `-YYYYMMDD` suffix.
pub fn normalize_model(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let after_slash = match lower.rfind('/') {
        Some(i) => lower[i + 1..].to_string(),
        None => lower,
    };
    if after_slash.len() >= 9 {
        let (head, tail) = after_slash.split_at(after_slash.len() - 9);
        if tail.starts_with('-') && tail[1..].chars().all(|c| c.is_ascii_digit()) {
            return head.to_string();
        }
    }
    after_slash
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
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

/// Fetch the latest LiteLLM snapshot (10s timeout), cache it, and rebuild.
/// On any fetch/parse failure, fall back to the cached file, then the bundled snapshot.
pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String> {
    let cache_file = cache_dir.join("model_prices.json");
    let fetched = ureq::get(LITELLM_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|resp| resp.into_string().ok());
    if let Some(body) = fetched {
        if let Ok(n) = rebuild_prices(conn, &body) {
            let _ = std::fs::create_dir_all(cache_dir);
            let _ = std::fs::write(&cache_file, &body);
            return Ok(n);
        }
    }
    if let Ok(body) = std::fs::read_to_string(&cache_file) {
        return rebuild_prices(conn, &body);
    }
    let bundled = include_str!("../resources/model_prices.json");
    rebuild_prices(conn, bundled)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
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
        if let Some(r) = self.prices.get(raw_model) {
            return Some(*r);
        }
        self.prices.get(&normalize_model(raw_model)).copied()
    }
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
}
