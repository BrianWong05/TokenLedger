use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use ts_rs::TS;

/// Providers whose normalized entry wins a collision over resellers — the model
/// labs that publish the Models, so their rate IS the List Price (CONTEXT.md).
///
/// Membership rule: a provider belongs here only if every entry it publishes is
/// its OWN Model. A cloud that also hosts other labs' Models must NOT be added,
/// or a reseller's rate would win the very collision this list exists to settle.
/// That rules out `tencent` (carries only DeepSeek Models), `volcengine`
/// (carries GLM and DeepSeek), `perplexity` (carries Mistral), and the cloud
/// platforms generally.
///
/// What the rule does NOT buy: two members can still publish the SAME Model, and
/// then key order decides. `gemini` and `vertex_ai-language-models` are both
/// Google and contest 27 normalized keys, 3 at different prices — pre-existing,
/// left alone here because settling it would move rates, which this change must
/// not. canonical_providers_never_disagree_on_a_price pins that so it cannot
/// quietly worsen, and every_canonical_provider_prices_something keeps inert
/// names out.
///
/// ponytail: a hand-curated list, only partly machine-checkable — a member that
/// resells another lab's Model at an identical price is invisible to both tests,
/// so additions need a human to apply the rule above. The durable fix is deriving
/// publisher identity structurally from the Model's own vendor rather than from a
/// list, which is what the publisher-rate tier does.
const CANONICAL: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "vertex_ai-language-models",
    "zai",
    "deepseek",
    "minimax",
    "xai",
    "mistral",
    "cohere",
    "ai21",
    "moonshot",
];

const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

/// The `catalog` values stored on a price row, and the origin strings the Pricing
/// tab renders (see the frontend's originLabel).
const CATALOG_LITELLM: &str = "litellm";
const CATALOG_OPENROUTER: &str = "openrouter";

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
    /// here. BOTH catalogs quote genuine $0 — OpenRouter's ":free" models, and
    /// LiteLLM entries where the publisher gives a Model away (zai/glm-4.7-flash).
    /// OpenRouter's are dropped (see openrouter_cost); LiteLLM's are kept, so ~120
    /// of its entries store 0.0 meaning "free" and are indistinguishable from "no
    /// rate". Telling the two apart needs nullable price columns end to end; add
    /// them if a free Model ever needs a real $0 Cost rather than reading as
    /// rate-less.
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

/// One OpenRouter price field. They arrive as decimal STRINGS. Only a strictly
/// positive rate is a rate; everything else maps to None:
/// - `"0"` — its ":free" models. Absent rates are stored as 0.0 here, so keeping
///   these would be indistinguishable from "no rate" anyway (see cache_gap).
/// - `"-1"` — its router pseudo-models (`openrouter/auto`, `openrouter/fusion`)
///   use -1 to mean "priced by whatever this routes to". Storing it would make
///   Cost go NEGATIVE.
/// - NaN, which fails `> 0.0` for free.
/// This is also what lets the both-None skip below cover "free model", "router
/// placeholder", and "no prices quoted" with one rule.
fn openrouter_cost(pricing: &serde_json::Value, key: &str) -> Option<f64> {
    let v: f64 = pricing.get(key)?.as_str()?.parse().ok()?;
    (v > 0.0).then_some(v)
}

/// Candidate rows from an OpenRouter catalog payload, keyed by its raw vendor-
/// prefixed id. Mirrors the LiteLLM loop's rule: an entry with neither a prompt
/// nor a completion price contributes nothing. A malformed payload yields no
/// rows rather than an error — losing the tier is survivable, and the caller
/// treats "unreadable" and "unreachable" identically (ADR-0003).
fn openrouter_rows(json: &str) -> Vec<(String, Row)> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(data) = root.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        // Load-bearing invariant, enforced rather than assumed (ADR-0003): exact
        // ids must stay OUT of the normalized keyspace, or write-time precedence
        // would stop matching the documented tier order. Every id OpenRouter has
        // ever served is vendor-prefixed; a bare one is dropped, not trusted.
        if !id.contains('/') {
            continue;
        }
        let Some(pricing) = entry.get("pricing") else {
            continue;
        };
        let input = openrouter_cost(pricing, "prompt");
        let output = openrouter_cost(pricing, "completion");
        if input.is_none() && output.is_none() {
            continue;
        }
        out.push((
            id.to_string(),
            Row {
                input,
                output,
                cache_read: openrouter_cost(pricing, "input_cache_read"),
                cw5m: openrouter_cost(pricing, "input_cache_write"),
                cw1h: openrouter_cost(pricing, "input_cache_write_1h"),
            },
        ));
    }
    out
}

/// Write one price row for `catalog`, LEAVING an existing row alone. Precedence
/// lives in the order rebuild_prices runs its passes: the first pass to claim a
/// Model key owns it, so every later pass must not overwrite. (Hence OR IGNORE
/// rather than the OR REPLACE this used when a single pass could only be
/// overwritten by the one authoritative exact-key pass that followed it.)
fn write_price_row(
    conn: &Connection,
    model: &str,
    row: &Row,
    catalog: &str,
) -> rusqlite::Result<()> {
    // 1h TTL falls back to the 5m rate when absent; null -> 0 at write time.
    let cw5m = row.cw5m.unwrap_or(0.0);
    let cw1h = row.cw1h.or(row.cw5m).unwrap_or(0.0);
    conn.execute(
        "INSERT OR IGNORE INTO prices \
         (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok, catalog) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            model,
            row.input.unwrap_or(0.0),
            row.output.unwrap_or(0.0),
            row.cache_read.unwrap_or(0.0),
            cw5m,
            cw1h,
            catalog,
        ],
    )?;
    Ok(())
}

/// Rebuild the `prices` table from the catalog snapshots. Writes an exact row
/// (model = the raw catalog key) for every entry with a non-null input OR output
/// cost, plus guarded normalized fallback rows, in ADR-0003 precedence order.
/// `openrouter_json` is None when that catalog could not be read at all, which
/// degrades to LiteLLM-only resolution. Returns the row count.
pub fn rebuild_prices(
    conn: &mut Connection,
    litellm_json: &str,
    openrouter_json: Option<&str>,
) -> Result<u64, String> {
    let openrouter = openrouter_json.map(openrouter_rows).unwrap_or_default();

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
    // The five-tier resolution order of ADR-0003, minus the Override tier (which
    // lives in RateMap, not here), run as ordered passes: write_price_row never
    // overwrites a key an earlier pass claimed, so the first claimant wins.
    // Exact raw-key matches outrank normalized ones, and an exact OpenRouter id
    // outranks a normalized LiteLLM match — a vendor-qualified exact hit is
    // stronger evidence than normalizing onto some reseller's row.
    for (model, row) in &exact {
        write_price_row(&tx, model, row, CATALOG_LITELLM).map_err(|e| e.to_string())?;
    }
    for (model, row) in &openrouter {
        write_price_row(&tx, model, row, CATALOG_OPENROUTER).map_err(|e| e.to_string())?;
    }
    for (model, (row, _)) in &norm {
        write_price_row(&tx, model, row, CATALOG_LITELLM).map_err(|e| e.to_string())?;
    }
    // ponytail: this pass takes first-in-payload when two OpenRouter ids share a
    // normalized tail, with none of the canonical-provider tiebreak the LiteLLM
    // normalized pass applies — the live 345-model payload has zero such
    // collisions, and a colliding pair is only reachable when LiteLLM covers the
    // Model not at all. Give it the same guard if OpenRouter ever ships one.
    for (model, row) in &openrouter {
        write_price_row(&tx, &normalize_model(model), row, CATALOG_OPENROUTER)
            .map_err(|e| e.to_string())?;
    }
    let count: u64 = tx
        .query_row("SELECT COUNT(*) FROM prices", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(count)
}

/// Fetch a catalog (10s timeout), caching a successful body under `cache_name` and
/// falling back to that cache when the fetch fails. Does NO DB work, so callers can
/// run the blocking network call outside the DB lock. None = the host has never
/// been reached and no cache exists yet.
fn fetch_or_cached(url: &str, cache_dir: &Path, cache_name: &str) -> Option<String> {
    let cache_file = cache_dir.join(cache_name);
    let fetched = ureq::get(url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|resp| resp.into_string().ok());
    if let Some(body) = fetched {
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(&cache_file, &body);
        return Some(body);
    }
    std::fs::read_to_string(&cache_file).ok()
}

/// The LiteLLM snapshot. Always yields JSON: a bundled snapshot backs the
/// fetch/cache chain, so even a first run with no network prices something.
pub fn load_prices_json(cache_dir: &Path) -> String {
    fetch_or_cached(LITELLM_URL, cache_dir, "model_prices.json")
        .unwrap_or_else(|| include_str!("../resources/model_prices.json").to_string())
}

/// The OpenRouter catalog. Unlike LiteLLM there is no bundled snapshot, so a
/// machine that has never reached OpenRouter gets None and resolves without the
/// tier — exactly the pre-OpenRouter behaviour (ADR-0003).
pub fn load_openrouter_json(cache_dir: &Path) -> Option<String> {
    fetch_or_cached(OPENROUTER_URL, cache_dir, "openrouter_models.json")
}

/// Fetch both catalogs and rebuild the prices table.
/// Production splits these two steps (fetch outside the DB lock); this convenience
/// wrapper is retained for the e2e test, hence test-only in non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String> {
    let litellm = load_prices_json(cache_dir);
    let openrouter = load_openrouter_json(cache_dir);
    rebuild_prices(conn, &litellm, openrouter.as_deref())
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
/// rates. `origin` is "litellm" | "openrouter", read from the row that matched.
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
    /// Model key -> (originating catalog, rates).
    prices: HashMap<String, (String, Rates)>,
    overrides: HashMap<String, Rates>,
}

impl RateMap {
    pub fn load(conn: &Connection) -> rusqlite::Result<RateMap> {
        let mut prices = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, \
             cache_write_5m_per_tok, cache_write_1h_per_tok, catalog FROM prices",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                // NULL catalog = a row written before v9, when LiteLLM was the
                // only catalog read. The next rebuild overwrites it anyway.
                r.get::<_, Option<String>>(6)?
                    .unwrap_or_else(|| CATALOG_LITELLM.to_string()),
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
            let (m, catalog, rt) = row?;
            prices.insert(m, (catalog, rt));
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
    /// normalized key, reporting which catalog matched. Two probes suffice
    /// because rebuild_prices' ordered passes already decided which pass owns
    /// each key, so a key's stored row IS its highest-precedence match.
    pub fn resolve_catalog(&self, raw_model: &str) -> Option<(&str, Rates)> {
        self.prices
            .get(raw_model)
            .or_else(|| self.prices.get(&normalize_model(raw_model)))
            .map(|(catalog, r)| (catalog.as_str(), *r))
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

/// Models in the Ledger that resolve to no rate at all and have not been looked
/// up yet this run — the trigger for re-reading the catalogs after a scan.
/// An Overridden Model never appears: an Override IS a rate. Unattributed Usage
/// has no Model, so it contributes nothing (ADR-0008). `attempted` is the
/// caller's in-memory record of names already tried, which is what keeps a Model
/// no catalog will ever carry to one fetch per run of the app.
pub fn models_needing_lookup(
    conn: &Connection,
    attempted: &HashSet<String>,
) -> rusqlite::Result<Vec<String>> {
    let rates = RateMap::load(conn)?;
    // No catalog loaded yet — the start-up refresh is still in flight, and a scan
    // can beat it. Every Model would look Unpriced, so the whole Ledger would be
    // marked attempted and a redundant second fetch would fire. A completed
    // rebuild always leaves rows (LiteLLM ships a bundled snapshot), so empty
    // here means "nothing read yet", never "nothing priced".
    if rates.prices.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt =
        conn.prepare("SELECT DISTINCT model FROM events WHERE model IS NOT NULL ORDER BY model")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let model = row?;
        if !attempted.contains(&model) && rates.resolve(&model).is_none() {
            out.push(model);
        }
    }
    Ok(out)
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
        let n = rebuild_prices(&mut conn, FIXTURE, None).unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn exact_wins_and_null_reseller_does_not_pollute() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None).unwrap();
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
        rebuild_prices(&mut conn, FIXTURE, None).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Not an exact key -> normalized to gemini-2.5-flash; canonical 3e-07
        // must win over the 2.5e-06 reseller.
        assert_eq!(rm.resolve("gemini-2.5-flash-20250101").unwrap().input, 3e-07);
    }

    #[test]
    fn claude_cache_rates_and_1h_fallback() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None).unwrap();
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
        rebuild_prices(&mut conn, FIXTURE, None).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("totally-unknown-model"), None);
    }

    #[test]
    fn override_wins_fills_none_and_applies_cache_write_both_ttls() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
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

    // The five-tier resolution fixtures (ADR-0003). LiteLLM covers `priced-full`
    // under its exact key, and `glm-5.2` ONLY via normalization of a reseller key
    // — the real-world shape that motivated putting openrouter-exact above
    // litellm-normalized (Cloudflare's rate is 1.8x the vendor's).
    const TIER_LITELLM: &str = r#"{
      "cloudflare/@cf/zai-org/glm-5.2": {
        "input_cost_per_token": 1.4e-06,
        "output_cost_per_token": 4.4e-06,
        "litellm_provider": "cloudflare"
      },
      "priced-full": {
        "input_cost_per_token": 3e-06,
        "output_cost_per_token": 6e-06,
        "litellm_provider": "openai"
      }
    }"#;

    // Field names verified against the real openrouter.ai/api/v1/models payload:
    // decimal STRINGS, and `:free` models priced at exactly "0".
    const TIER_OPENROUTER: &str = r#"{
      "data": [
        { "id": "z-ai/glm-5.2",
          "pricing": { "prompt": "0.0000007742", "completion": "0.0000024332",
                       "input_cache_read": "0.00000014378" } },
        { "id": "anthropic/claude-opus-5",
          "pricing": { "prompt": "0.000005", "completion": "0.000025",
                       "input_cache_read": "0.0000005", "input_cache_write": "0.00000625",
                       "input_cache_write_1h": "0.00001" } },
        { "id": "openai/priced-full",
          "pricing": { "prompt": "0.00009", "completion": "0.00009" } },
        { "id": "poolside/laguna-s:free",
          "pricing": { "prompt": "0", "completion": "0" } },
        { "id": "openrouter/auto",
          "pricing": { "prompt": "-1", "completion": "-1" } },
        { "id": "bare-id-no-vendor",
          "pricing": { "prompt": "0.000001", "completion": "0.000002" } },
        { "id": "vendor/no-prices", "pricing": {} }
      ]
    }"#;

    fn tier_map(conn: &mut Connection) -> RateMap {
        rebuild_prices(conn, TIER_LITELLM, Some(TIER_OPENROUTER)).unwrap();
        RateMap::load(conn).unwrap()
    }

    // A publisher and a reseller carrying the SAME Model under prefixed keys, so
    // neither claims the normalized key outright and the collision guard decides.
    // Key order matters: serde_json sorts object keys, so "azure_ai/..." is merged
    // before "zai/..." — the adversarial direction, where the reseller would win
    // on order alone.
    const GUARD_RESELLER_FIRST: &str = r#"{
      "azure_ai/lab-model": {
        "input_cost_per_token": 4e-06,
        "output_cost_per_token": 8e-06,
        "litellm_provider": "azure_ai"
      },
      "zai/lab-model": {
        "input_cost_per_token": 1e-06,
        "output_cost_per_token": 2e-06,
        "litellm_provider": "zai"
      }
    }"#;

    // The mirror: the publisher's key sorts FIRST. Guards against a future change
    // that inverts the tiebreak and only looks correct in one direction.
    const GUARD_PUBLISHER_FIRST: &str = r#"{
      "deepseek/lab-model": {
        "input_cost_per_token": 1e-06,
        "output_cost_per_token": 2e-06,
        "litellm_provider": "deepseek"
      },
      "zzz-reseller/lab-model": {
        "input_cost_per_token": 4e-06,
        "output_cost_per_token": 8e-06,
        "litellm_provider": "zzz-reseller"
      }
    }"#;

    // The one place two canonical providers legitimately contest keys: both are
    // Google surfaces for the same Models. Pre-existing and priced differently;
    // see the CANONICAL doc comment.
    const KNOWN_CANONICAL_OVERLAP: [&str; 2] = ["gemini", "vertex_ai-language-models"];

    #[test]
    fn canonical_providers_never_disagree_on_a_price() {
        // The safety property the guard actually needs, checked against the bundled
        // snapshot rather than asserted in a comment: when two members contest a
        // normalized key, key order picks the winner, so they had better quote the
        // same rate. Note what this does NOT catch — a member reselling another
        // lab's Model at the SAME price is invisible here, which is why the
        // membership rule on CANONICAL stays a human check.
        let root: serde_json::Value =
            serde_json::from_str(include_str!("../resources/model_prices.json")).unwrap();
        // (provider, input, output) per normalized key.
        type Quote<'a> = (&'a str, Option<f64>, Option<f64>);
        let mut by_key: HashMap<String, Vec<Quote>> = HashMap::new();
        for (key, entry) in root.as_object().unwrap() {
            let Some(p) = entry.get("litellm_provider").and_then(|v| v.as_str()) else {
                continue;
            };
            if !CANONICAL.contains(&p) {
                continue;
            }
            let (i, o) = (cost(entry, "input_cost_per_token"), cost(entry, "output_cost_per_token"));
            if i.is_none() && o.is_none() {
                continue; // rebuild_prices skips these, so they can never collide
            }
            by_key.entry(normalize_model(key)).or_default().push((p, i, o));
        }

        for (key, rows) in &by_key {
            let providers: HashSet<&str> = rows.iter().map(|r| r.0).collect();
            if providers.len() < 2 {
                continue;
            }
            let prices: HashSet<(String, String)> =
                rows.iter().map(|r| (format!("{:?}", r.1), format!("{:?}", r.2))).collect();
            if prices.len() < 2 {
                continue; // same Model, same rate — nothing for key order to get wrong
            }
            let unexpected: Vec<&str> = providers
                .iter()
                .copied()
                .filter(|p| !KNOWN_CANONICAL_OVERLAP.contains(p))
                .collect();
            assert!(
                unexpected.is_empty(),
                "canonical providers {unexpected:?} quote different prices for `{key}`, so key \
                 order rather than the guard decides which one a Model resolves to. Either one \
                 of them is reselling a Model it does not publish and should leave CANONICAL, \
                 or it is a second surface of the same publisher and belongs in \
                 KNOWN_CANONICAL_OVERLAP."
            );
        }
    }

    #[test]
    fn every_canonical_provider_prices_something() {
        // A provider with no priced entries is inert: rebuild_prices skips null-
        // priced entries before the guard runs, so listing it is decoration.
        let root: serde_json::Value =
            serde_json::from_str(include_str!("../resources/model_prices.json")).unwrap();
        let obj = root.as_object().unwrap();
        for p in CANONICAL {
            let priced = obj.values().filter(|e| {
                e.get("litellm_provider").and_then(|v| v.as_str()) == Some(p)
                    && (cost(e, "input_cost_per_token").is_some()
                        || cost(e, "output_cost_per_token").is_some())
            });
            assert!(priced.count() > 0, "CANONICAL lists `{p}`, which prices nothing");
        }
    }

    #[test]
    fn a_publisher_row_beats_a_reseller_row_whichever_sorts_first() {
        // Neither fixture has a bare "lab-model" key, so the normalized key is
        // genuinely contested and only the guard settles it. Resolve a raw name
        // that is nobody's exact key, forcing the normalized lookup.
        for (fixture, label) in [
            (GUARD_RESELLER_FIRST, "reseller sorts first"),
            (GUARD_PUBLISHER_FIRST, "publisher sorts first"),
        ] {
            let (_d, mut conn) = test_conn();
            rebuild_prices(&mut conn, fixture, None).unwrap();
            let rm = RateMap::load(&conn).unwrap();
            let r = rm.resolve("some-host/lab-model").unwrap();
            assert_eq!(r.input, 1e-06, "publisher's rate must win ({label})");
            assert_eq!(r.output, 2e-06, "publisher's rate must win ({label})");
        }
    }

    #[test]
    fn openrouter_exact_outranks_litellm_normalized() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        // The raw Model name matches an OpenRouter id exactly; LiteLLM reaches it
        // only by normalizing a Cloudflare reseller key. Exact evidence wins.
        let (origin, r) = rm.resolve_catalog("z-ai/glm-5.2").unwrap();
        assert_eq!(origin, "openrouter");
        assert_eq!(r.input, 7.742e-07);
        assert_eq!(r.output, 2.4332e-06);
    }

    #[test]
    fn openrouter_normalized_fills_a_litellm_gap() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        // No LiteLLM coverage at all; OpenRouter's vendor-prefixed id normalizes
        // onto the raw name. All four token categories carry across.
        let (origin, r) = rm.resolve_catalog("claude-opus-5").unwrap();
        assert_eq!(origin, "openrouter");
        assert_eq!(r.input, 5e-06);
        assert_eq!(r.output, 2.5e-05);
        assert_eq!(r.cache_read, 5e-07);
        assert_eq!(r.cache_write_5m, 6.25e-06);
        assert_eq!(r.cache_write_1h, 1e-05);
    }

    #[test]
    fn litellm_exact_outranks_openrouter() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        let (origin, r) = rm.resolve_catalog("priced-full").unwrap();
        assert_eq!(origin, "litellm");
        assert_eq!(r.input, 3e-06, "LiteLLM's exact key, not OpenRouter's 9e-05");
    }

    #[test]
    fn override_outranks_both_catalogs() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER)).unwrap();
        set_override(
            &conn,
            "z-ai/glm-5.2",
            OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: None },
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("z-ai/glm-5.2").unwrap().input, 9e-06);
        // resolve_catalog still reports the catalog match, ignoring the Override.
        assert_eq!(rm.resolve_catalog("z-ai/glm-5.2").unwrap().0, "openrouter");
    }

    #[test]
    fn zero_priced_openrouter_entries_are_skipped() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        // A ":free" model priced at "0" is dropped. This is a KNOWN, accepted loss
        // against CONTEXT.md's Unpriced rule ("a genuinely free Model and an
        // unknown price never look alike") — storing $0 would break that rule too,
        // since an absent rate is already stored as 0.0, and telling them apart
        // needs nullable price columns end to end (see Rates::cache_gap).
        assert_eq!(rm.resolve_catalog("poolside/laguna-s:free"), None);
        // A router placeholder priced at "-1" must never become a negative rate.
        assert_eq!(rm.resolve_catalog("openrouter/auto"), None);
        // And an entry carrying no prices at all.
        assert_eq!(rm.resolve_catalog("vendor/no-prices"), None);
    }

    #[test]
    fn an_absent_openrouter_catalog_degrades_to_litellm_only() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, TIER_LITELLM, None).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Exactly today's behaviour: the normalized Cloudflare row is the only match.
        let (origin, r) = rm.resolve_catalog("z-ai/glm-5.2").unwrap();
        assert_eq!(origin, "litellm");
        assert_eq!(r.input, 1.4e-06);
        assert_eq!(rm.resolve_catalog("claude-opus-5"), None);
    }

    #[test]
    fn an_openrouter_model_without_cache_rates_is_cache_estimated() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        // glm-5.2 prices cache READS but not cache writes -> Cache-Estimated.
        let r = rm.resolve("z-ai/glm-5.2").unwrap();
        assert!(!r.cache_gap(100, 0, 0), "cache reads are priced");
        assert!(r.cache_gap(0, 100, 0), "cache writes are not");
    }

    #[test]
    fn the_parser_enforces_the_disjoint_key_space_invariant() {
        // Write-time precedence over ONE Model keyspace equals a five-tier
        // read-time resolve only while exact ids and normalized keys cannot
        // collide: every OpenRouter id is vendor-prefixed, and normalization
        // strips through the last '/'. The parser ENFORCES the first half rather
        // than trusting the payload, so a bare id can never enter the normalized
        // keyspace and silently outrank a LiteLLM exact match.
        let ids: Vec<String> = openrouter_rows(TIER_OPENROUTER).into_iter().map(|(m, _)| m).collect();
        assert!(!ids.is_empty(), "fixture must yield rows for this to mean anything");
        assert!(
            !ids.iter().any(|id| id == "bare-id-no-vendor"),
            "a non-vendor-prefixed id must be dropped, not stored as an exact key"
        );
        for id in &ids {
            assert!(id.contains('/'), "stored OpenRouter id {id} is not vendor-prefixed");
            assert!(!normalize_model(id).contains('/'), "normalized {id} kept a separator");
        }
    }

    #[test]
    fn a_malformed_openrouter_payload_yields_no_rows() {
        assert!(openrouter_rows("not json at all").is_empty());
        assert!(openrouter_rows(r#"{"nope": 1}"#).is_empty());
    }

    #[test]
    fn catalog_origin_is_read_from_storage_not_hardcoded() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
        // The rebuild must RECORD its catalog, not leave it for the read side to
        // assume: RateMap reads a NULL catalog as "litellm", so without this every
        // assertion below would still pass if write_price_row bound NULL.
        let stored: Option<String> = conn
            .query_row("SELECT catalog FROM prices WHERE model = 'priced-full'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored.as_deref(), Some(CATALOG_LITELLM));

        // A row attributed to another catalog must report THAT catalog — the origin
        // is data, not a constant chosen by the resolver.
        conn.execute(
            "INSERT OR REPLACE INTO prices (model, input_per_tok, output_per_tok, catalog) \
             VALUES ('from-elsewhere', 1e-06, 2e-06, 'openrouter')",
            [],
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve_catalog("from-elsewhere").unwrap().0, "openrouter");
        assert_eq!(rm.resolve_catalog("priced-full").unwrap().0, CATALOG_LITELLM);

        // And the origin reaches the Pricing tab unchanged.
        seed_event(&conn, "from-elsewhere", "hermes");
        let list = model_pricing(&conn).unwrap();
        let row = list.iter().find(|m| m.model == "from-elsewhere").unwrap();
        assert_eq!(row.catalog.as_ref().unwrap().origin, "openrouter");
    }

    #[test]
    fn a_price_row_predating_the_catalog_column_reads_as_litellm() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
        // What the v9 migration leaves behind: a row with no catalog recorded.
        // Every pre-v9 row came from the one catalog then read, so it reports that.
        conn.execute(
            "INSERT OR REPLACE INTO prices (model, input_per_tok, output_per_tok, catalog) \
             VALUES ('legacy-row', 1e-06, 2e-06, NULL)",
            [],
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve_catalog("legacy-row").unwrap().0, CATALOG_LITELLM);
    }

    #[test]
    fn models_needing_lookup_returns_an_unpriced_model_once_and_skips_the_rest() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
        seed_event(&conn, "priced-full", "claude"); // has a catalog List Price
        seed_event(&conn, "brand-new-model", "claude"); // no rate anywhere
        seed_event(&conn, "hand-priced", "hermes"); // no catalog rate, but Overridden
        set_override(
            &conn,
            "hand-priced",
            OverrideRates { input: Some(1e-06), output: None, cache_read: None, cache_write: None },
        )
        .unwrap();
        // Unattributed Usage has no Model, so it can never need a lookup (ADR-0008).
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, input_tokens, source_file) \
             VALUES ('pi:tool-result:1', 'pi', 0, NULL, 100, 'pi.jsonl')",
            [],
        )
        .unwrap();

        let mut attempted = HashSet::new();
        let fresh = models_needing_lookup(&conn, &attempted).unwrap();
        assert_eq!(fresh, vec!["brand-new-model".to_string()]);

        // Recording the attempt stops it coming back: one lookup per Model per run,
        // so a Model no catalog will ever carry cannot fetch on every scan.
        attempted.extend(fresh);
        assert!(models_needing_lookup(&conn, &attempted).unwrap().is_empty());
    }

    #[test]
    fn models_needing_lookup_is_empty_before_any_catalog_has_loaded() {
        let (_d, conn) = test_conn();
        // Cold start: the frontend's first scan can land before the start-up
        // refresh finishes, leaving prices empty. Every Model would look Unpriced,
        // so the whole Ledger would be marked attempted and a redundant second
        // fetch would fire. With no catalog loaded there is nothing to conclude.
        seed_event(&conn, "priced-full", "claude");
        seed_event(&conn, "brand-new-model", "claude");
        assert!(models_needing_lookup(&conn, &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn models_needing_lookup_is_empty_for_an_empty_ledger() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
        assert!(models_needing_lookup(&conn, &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn override_set_delete_roundtrip_via_model_pricing() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None).unwrap();
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
