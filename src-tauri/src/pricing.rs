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
/// ORDER IS LOAD-BEARING. Two members can publish the SAME Model, and the one
/// listed FIRST sets its List Price (see canonical_rank). `gemini` and
/// `vertex_ai-language-models` are the case that forces the rule: two Google
/// surfaces for one Model, contesting 27 normalized keys and quoting different
/// rates on 3 of them. `gemini` leads because the Gemini API is the surface
/// these Sources actually bill against — Gemini CLI and Antigravity record bare
/// Model ids and default to it, while Vertex is opt-in behind a GCP project. So
/// where Google publishes two rate cards for one Model, the direct one is the
/// Model's List Price and Vertex's is the platform's. Vertex stays a member: its
/// rate must still beat a reseller's on the Models only Vertex carries.
///
/// The publisher-rate tier cannot settle this one. It identifies a publisher by
/// matching a host's tag vendor to the Model's own vendor in OpenRouter's host
/// listing, and Google does not serve Gemini there (ADR-0009), so no Gemini
/// Model ever reaches that tier and this list stays the only thing deciding.
///
/// ponytail: a hand-curated list, only partly machine-checkable — a member that
/// resells another lab's Model at an identical price is invisible to the tests,
/// so additions need a human to apply the rule above. The durable fix is deriving
/// publisher identity structurally from the Model's own vendor rather than from a
/// list, which is what the publisher-rate tier does where it can reach.
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

/// Where `provider` ranks among the publishers — **lower wins**, non-members
/// last. The whole reason CANONICAL is an ordered slice and not a set: it settles
/// a collision between two publishers as well as one between a publisher and a
/// reseller, so no rate is ever picked by which key happens to sort first.
fn canonical_rank(provider: &str) -> usize {
    CANONICAL.iter().position(|p| *p == provider).unwrap_or(usize::MAX)
}

const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

/// The `catalog` values stored on a price row, and the origin strings the Pricing
/// tab renders (see the frontend's originLabel).
const CATALOG_LITELLM: &str = "litellm";
const CATALOG_OPENROUTER: &str = "openrouter";

/// How much a stored row is worth believing — **lower is better**, so this is a
/// rank, not ADR-0009's tier numbering. A publisher's own rate is the Model's
/// List Price; LiteLLM is a published rate but not necessarily the publisher's;
/// a Routed Rate is set by nobody.
///
/// The `catalog` column carries a publisher's NAME when the rate came from one,
/// so anything that is not a catalog id is a publisher. That default is
/// deliberate: the only way to land here with an unrecognised value is a row this
/// build wrote, and the rebuild wipes and rewrites every row on every refresh.
fn source_rank(catalog: &str) -> u8 {
    match catalog {
        CATALOG_OPENROUTER => 2,
        CATALOG_LITELLM => 1,
        _ => 0,
    }
}

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
/// treats "unreadable" and "unreachable" identically (ADR-0009).
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
        // Still load-bearing under ADR-0009, for a different reason than it was
        // under the ADR it superseded: precedence is now settled at read time, so
        // a bare id would not out-RANK a catalog row — but it would land on the
        // same key and, first pass winning, evict it entirely, leaving only a
        // Routed Rate where a published one existed. Every id OpenRouter has ever
        // served is vendor-prefixed; a bare one is dropped, not trusted.
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

/// A Model's List Price as its own publisher quotes it (CONTEXT.md), plus who
/// that publisher is and when it was read. Cached to disk between runs, so it
/// serialises; `fetched_at` is unix seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherRate {
    pub model_id: String,
    pub publisher: String,
    #[serde(flatten)]
    rates: Row,
}

/// One cached lookup: when we last asked, and what came back.
///
/// `rate: None` is a real answer — "this publisher does not host this Model" —
/// not an absence of one, and recording it is what keeps such Models from being
/// re-asked on every single refresh. Four of sixteen Models in a real Ledger
/// answer this way, and left unrecorded they would sort ahead of everything else
/// forever and starve the Models that do have a rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublisherLookup {
    fetched_at: i64,
    rate: Option<PublisherRate>,
}

/// The publisher's own entry in a per-Model host listing.
///
/// The publisher is found structurally rather than from a list of who publishes
/// what: its host tag carries the same vendor as the Model identifier, so
/// `z-ai/fp8` is the publisher of `z-ai/glm-5.2` while `cloudflare` is not. That
/// is what makes this robust where the CANONICAL allowlist is not — it needs no
/// maintenance as new labs appear.
///
/// None when the publisher does not host the Model at all (Google does not serve
/// Gemini here), or the payload is unusable. Hosts are listed cheapest-first and
/// a publisher may offer several tiers, so the first match is taken: the
/// standard tier, not a premium `fast` variant.
fn publisher_rate(json: &str) -> Option<PublisherRate> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = root.get("data")?;
    let model_id = data.get("id")?.as_str()?;
    let publisher = model_id.split('/').next()?;

    for endpoint in data.get("endpoints")?.as_array()? {
        let tag = endpoint.get("tag").and_then(|v| v.as_str()).unwrap_or_default();
        if tag.split('/').next() != Some(publisher) {
            continue;
        }
        let Some(pricing) = endpoint.get("pricing") else {
            continue;
        };
        let input = openrouter_cost(pricing, "prompt");
        let output = openrouter_cost(pricing, "completion");
        if input.is_none() && output.is_none() {
            continue;
        }
        return Some(PublisherRate {
            model_id: model_id.to_string(),
            publisher: endpoint
                .get("provider_name")
                .and_then(|v| v.as_str())
                .unwrap_or(publisher)
                .to_string(),
            rates: Row {
                input,
                output,
                cache_read: openrouter_cost(pricing, "input_cache_read"),
                cw5m: openrouter_cost(pricing, "input_cache_write"),
                cw1h: openrouter_cost(pricing, "input_cache_write_1h"),
            },
        });
    }
    None
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
/// cost, plus guarded normalized fallback rows, in ADR-0009 precedence order.
/// `openrouter_json` is None when that catalog could not be read at all, which
/// degrades to LiteLLM-only resolution. Returns the row count.
pub fn rebuild_prices(
    conn: &mut Connection,
    litellm_json: &str,
    openrouter_json: Option<&str>,
    publishers: &[PublisherRate],
) -> Result<u64, String> {
    let openrouter = openrouter_json.map(openrouter_rows).unwrap_or_default();

    let root: serde_json::Value =
        serde_json::from_str(litellm_json).map_err(|e| format!("parse litellm json: {e}"))?;
    let obj = root
        .as_object()
        .ok_or_else(|| "litellm json is not an object".to_string())?;

    let mut exact: Vec<(String, Row, usize)> = Vec::new(); // (key, row, publisher rank)
    let mut norm: HashMap<String, (Row, usize)> = HashMap::new(); // key -> (row, best rank)

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
        let rank = entry
            .get("litellm_provider")
            .and_then(|v| v.as_str())
            .map(canonical_rank)
            .unwrap_or(usize::MAX);
        exact.push((key.clone(), row.clone(), rank));

        let nkey = normalize_model(key);
        match norm.get_mut(&nkey) {
            None => {
                norm.insert(nkey, (row, rank));
            }
            Some((existing, existing_rank)) => {
                // Strictly better rank wins, so two publishers settle by CANONICAL
                // order and two non-members (both usize::MAX) still settle by key
                // order — nobody outranks anybody there.
                let new_wins = rank < *existing_rank;
                if new_wins {
                    // New (better-ranked) row wins; keep an old non-null field only where new is null.
                    existing.input = row.input.or(existing.input);
                    existing.output = row.output.or(existing.output);
                    existing.cache_read = row.cache_read.or(existing.cache_read);
                    existing.cw5m = row.cw5m.or(existing.cw5m);
                    existing.cw1h = row.cw1h.or(existing.cw1h);
                    *existing_rank = rank;
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

    // A publisher quotes its Model's List Price but not always every field of it.
    // Fill the gaps from the primary catalog — same "never overwrite a non-null
    // with a null" rule the normalized merge above uses — so adopting a publisher
    // rate can never LOSE a rate the catalog already published (a Model whose 1h
    // cache-write TTL only the catalog carries would otherwise fall back to its
    // 5m rate and undercount those tokens).
    let published: Vec<(&PublisherRate, Row)> = publishers
        .iter()
        .map(|p| {
            let mut row = p.rates.clone();
            let nkey = normalize_model(&p.model_id);
            if let Some(fill) = exact.iter().find(|(k, ..)| *k == p.model_id).map(|(_, r, _)| r)
                .or_else(|| norm.get(&nkey).map(|(r, _)| r))
            {
                row.input = row.input.or(fill.input);
                row.output = row.output.or(fill.output);
                row.cache_read = row.cache_read.or(fill.cache_read);
                row.cw5m = row.cw5m.or(fill.cw5m);
                row.cw1h = row.cw1h.or(fill.cw1h);
            }
            (p, row)
        })
        .collect();

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM prices", []).map_err(|e| e.to_string())?;
    // The resolution order, minus the Override tier (which lives in RateMap, not
    // here), run as ordered passes: write_price_row never overwrites a key an
    // earlier pass claimed, so the first claimant wins.
    //
    //   publisher rate -> primary catalog -> routed rate
    //
    // A publisher's own rate IS the Model's List Price, so it leads. The routed
    // rate trails everything: no publisher sets it, it is blended across every
    // host and moves with their discounts, and it exists only so a Model with no
    // published rate anywhere is priced rather than Unpriced.
    for (p, row) in &published {
        write_price_row(&tx, &p.model_id, row, &p.publisher).map_err(|e| e.to_string())?;
    }
    for (p, row) in &published {
        write_price_row(&tx, &normalize_model(&p.model_id), row, &p.publisher)
            .map_err(|e| e.to_string())?;
    }
    for (model, row, rank) in &exact {
        // A Model's bare name is also its normalized name, so an exact key spelled
        // that way claims the slot the normalized merge is for — and claims it
        // whatever the catalog's spelling convention happens to imply about whose
        // rate it is. LiteLLM spells Google's bare Gemini keys as Vertex's and
        // prefixes the direct API's `gemini/`, so without this a Ledger's
        // `gemini-2.0-flash-001` would price at the Vertex rate no matter what
        // CANONICAL says. Yield the slot to a better-ranked publisher's merged
        // row; a prefixed key never matches a normalized one, so its own row is
        // untouched.
        if norm.get(model).is_some_and(|(_, best)| best < rank) {
            continue;
        }
        write_price_row(&tx, model, row, CATALOG_LITELLM).map_err(|e| e.to_string())?;
    }
    for (model, (row, _)) in &norm {
        write_price_row(&tx, model, row, CATALOG_LITELLM).map_err(|e| e.to_string())?;
    }
    for (model, row) in &openrouter {
        write_price_row(&tx, model, row, CATALOG_OPENROUTER).map_err(|e| e.to_string())?;
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
/// tier — exactly the pre-OpenRouter behaviour (ADR-0009).
pub fn load_openrouter_json(cache_dir: &Path) -> Option<String> {
    fetch_or_cached(OPENROUTER_URL, cache_dir, "openrouter_models.json")
}

/// The OpenRouter ids serving Models the Ledger actually holds — the only Models
/// a publisher rate is ever fetched for. A raw Model name matches an id exactly
/// or after normalizing, the same two ways the catalogs match.
pub fn ledger_publisher_targets(
    conn: &Connection,
    openrouter_json: &str,
) -> rusqlite::Result<Vec<String>> {
    let ids: Vec<String> = openrouter_rows(openrouter_json).into_iter().map(|(id, _)| id).collect();
    let by_exact: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let by_norm: HashMap<String, &str> =
        ids.iter().map(|s| (normalize_model(s), s.as_str())).collect();

    let mut stmt = conn.prepare("SELECT DISTINCT model FROM events WHERE model IS NOT NULL")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        let model = row?;
        let hit = by_exact
            .get(model.as_str())
            .copied()
            .or_else(|| by_norm.get(&normalize_model(&model)).copied());
        if let Some(id) = hit {
            if !out.iter().any(|m| m == id) {
                out.push(id.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// How long a publisher rate stays fresh. Publishers change their list prices
/// rarely — far more rarely than the app refreshes — so re-reading one that was
/// read today buys nothing, and a slightly old publisher rate is still a
/// publisher rate. This is what makes steady state nearly free however large the
/// Ledger grows: most refreshes find everything fresh and issue no requests.
const PUBLISHER_RATE_MAX_AGE: i64 = 24 * 60 * 60;

/// Most publisher reads any one refresh will issue. The staleness window cannot
/// help on a cold cache, where every Model is due at once, so this bounds that:
/// a large Ledger fills in across several refreshes instead of firing hundreds of
/// requests at launch. Chosen to keep a cold start to a handful of refreshes at
/// realistic Ledger sizes, not from a measurement — reads run eight at a time and
/// each can wait out a 10s timeout, so this caps count, not wall-clock.
const PUBLISHER_READ_CEILING: usize = 50;

/// Which publisher rates to read now, and how many were left for later.
struct PublisherReads {
    due: Vec<String>,
    deferred: usize,
}

/// Decide which Models to read a publisher rate for, bounded twice: by the
/// staleness window, and by a ceiling on one refresh's worth of requests.
///
/// Never-asked Models sort first — nothing is known about them at all — then
/// least-recently-asked, so repeated refreshes converge on full coverage instead
/// of re-reading the same subset. Ties keep target order, which is itself sorted,
/// so the choice is stable across refreshes. Takes `now` and `ceiling` as
/// arguments rather than reading a clock or a constant, so the whole decision is
/// testable without threads or network.
fn publisher_reads_due(
    targets: &[String],
    cached: &HashMap<String, PublisherLookup>,
    now: i64,
    ceiling: usize,
) -> PublisherReads {
    // saturating: fetched_at is deserialized from a file a user can edit, and an
    // absurd value must not panic a debug build.
    let mut due: Vec<&String> = targets
        .iter()
        .filter(|id| {
            cached
                .get(*id)
                .is_none_or(|l| now.saturating_sub(l.fetched_at) >= PUBLISHER_RATE_MAX_AGE)
        })
        .collect();
    due.sort_by_key(|id| cached.get(*id).map_or(i64::MIN, |l| l.fetched_at));

    let deferred = due.len().saturating_sub(ceiling);
    PublisherReads {
        due: due.into_iter().take(ceiling).cloned().collect(),
        deferred,
    }
}

/// Fold read results into the cache, stamped with when they were read.
///
/// Every entry here is an answer, including `None` — "the host listing was read
/// and the publisher is not in it". Recording that is what stops such a Model
/// looking never-asked forever, sorting ahead of everything else on every
/// refresh, and starving the Models that do have a rate. Requests that FAILED
/// never reach this function, so a network blink is not mistaken for an answer.
fn record_lookups(
    cached: &mut HashMap<String, PublisherLookup>,
    fetched: Vec<(String, Option<PublisherRate>)>,
    now: i64,
) {
    for (model_id, rate) in fetched {
        cached.insert(model_id, PublisherLookup { fetched_at: now, rate });
    }
}

/// Read each Model's publisher rate, one request per Model. Does NO DB work, for
/// the same reason the catalog loaders don't.
///
/// Every lookup is cached to one snapshot file, INCLUDING the answer "this
/// publisher does not host this Model" — that is a real answer that ages like any
/// other. A request that fails is not: nothing is recorded, so the Model keeps
/// whatever rate it had and is asked again next refresh rather than being written
/// off for a day because the network blinked.
///
/// Bounded by publisher_reads_due: only stale or never-asked Models are read, at
/// most PUBLISHER_READ_CEILING of them per refresh. Anything deferred keeps
/// whatever rate it already had — a cached publisher rate, or a catalog rate —
/// so bounding the work can never turn a priced Model Unpriced. Returns every
/// rate we hold for `ids`, read this time or not.
pub fn load_publisher_rates(cache_dir: &Path, ids: &[String]) -> Vec<PublisherRate> {
    let cache_file = cache_dir.join("openrouter_publishers.json");
    let mut cached: HashMap<String, PublisherLookup> = std::fs::read_to_string(&cache_file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let reads = publisher_reads_due(ids, &cached, now, PUBLISHER_READ_CEILING);
    if reads.deferred > 0 {
        // ponytail: stderr is the only channel this app has — it has no logger,
        // and a packaged build discards it. Partial coverage still must not pass
        // silently, so say it here until there is somewhere in the interface to
        // say it; the Pricing tab is the right home once #52 lands.
        eprintln!(
            "tokenledger: read {} publisher rates, deferred {} to a later refresh",
            reads.due.len(),
            reads.deferred
        );
    }
    // NOT `ids` — only what is due is read, but every id's rate is returned below.
    let to_read = &reads.due;

    // ponytail: fixed fan-out over scoped threads rather than an async runtime —
    // ureq is blocking and this is the only concurrent fetch in the app.
    const LANES: usize = 8;
    // Per id: Some(answer) when the host listing was read — the answer itself may
    // be None, meaning no publisher — and nothing at all when the request failed.
    let fetched: Vec<(String, Option<PublisherRate>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = to_read
            .chunks(to_read.len().div_ceil(LANES).max(1))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|id| {
                            let url = format!("{OPENROUTER_URL}/{id}/endpoints");
                            let body = ureq::get(&url)
                                .timeout(std::time::Duration::from_secs(10))
                                .call()
                                .ok()?
                                .into_string()
                                .ok()?;
                            Some((id.clone(), publisher_rate(&body)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    });

    record_lookups(&mut cached, fetched, now);
    let _ = std::fs::create_dir_all(cache_dir);
    if let Ok(body) = serde_json::to_string(&cached) {
        let _ = std::fs::write(&cache_file, body);
    }
    // Every requested Model we hold a rate for, NOT just the ones read this time:
    // a deferred Model keeps the publisher rate it already had rather than falling
    // back to LiteLLM or a Routed Rate until its turn comes round.
    ids.iter().filter_map(|id| cached.get(id).and_then(|l| l.rate.clone())).collect()
}

/// Fetch both catalogs and rebuild the prices table.
/// Production splits these two steps (fetch outside the DB lock); this convenience
/// wrapper is retained for the e2e test, hence test-only in non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String> {
    let litellm = load_prices_json(cache_dir);
    let openrouter = load_openrouter_json(cache_dir);
    let targets = match openrouter.as_deref() {
        Some(json) => ledger_publisher_targets(conn, json).unwrap_or_default(),
        None => Vec::new(),
    };
    let publishers = load_publisher_rates(cache_dir, &targets);
    rebuild_prices(conn, &litellm, openrouter.as_deref(), &publishers)
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

/// A catalog List Price match: which catalog it came from (ADR-0009) and its
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

    /// The catalog tier of `resolve`, ignoring overrides.
    ///
    /// Two keys can answer for one Model — the raw name and its normalized form —
    /// and the better SOURCE wins, not the better-matching key. Probing exact
    /// first would always hand a Routed Rate the win over a catalog rate, because
    /// a routed id is a vendor-prefixed exact key while a catalog's fallback is
    /// only ever reachable normalized. rebuild_prices' pass order settles who owns
    /// each key; this settles which of the two keys to believe.
    pub fn resolve_catalog(&self, raw_model: &str) -> Option<(&str, Rates)> {
        let exact = self.prices.get(raw_model);
        let normalized = self.prices.get(&normalize_model(raw_model));
        let best = match (exact, normalized) {
            // Ties go to the exact key: same source, more specific match.
            (Some(e), Some(n)) if source_rank(&n.0) < source_rank(&e.0) => n,
            (Some(e), _) => e,
            (None, n) => n?,
        };
        Some((best.0.as_str(), best.1))
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
        let n = rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn exact_wins_and_null_reseller_does_not_pollute() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Not an exact key -> normalized to gemini-2.5-flash; canonical 3e-07
        // must win over the 2.5e-06 reseller.
        assert_eq!(rm.resolve("gemini-2.5-flash-20250101").unwrap().input, 3e-07);
    }

    #[test]
    fn claude_cache_rates_and_1h_fallback() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("totally-unknown-model"), None);
    }

    #[test]
    fn override_wins_fills_none_and_applies_cache_write_both_ttls() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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

    // The five-tier resolution fixtures (ADR-0009). LiteLLM covers `priced-full`
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
      },
      "lab-fable": {
        "input_cost_per_token": 1e-05,
        "output_cost_per_token": 5e-05,
        "cache_read_input_token_cost": 1e-06,
        "cache_creation_input_token_cost": 1.25e-05,
        "cache_creation_input_token_cost_above_1hr": 2e-05,
        "litellm_provider": "anthropic"
      }
    }"#;

    // The publisher quotes everything EXCEPT the 1h cache-write TTL, which only
    // the primary catalog publishes — the real claude-fable-5 shape.
    const HOSTS_LAB_FABLE: &str = r#"{
      "data": {
        "id": "anthropic/lab-fable",
        "endpoints": [
          { "provider_name": "Anthropic", "tag": "anthropic",
            "pricing": { "prompt": "0.00001", "completion": "0.00005",
                         "input_cache_read": "0.000001", "input_cache_write": "0.0000125" } }
        ]
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
        rebuild_prices(conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[]).unwrap();
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

    #[test]
    fn the_first_listed_canonical_provider_sets_a_contested_price() {
        // When two publishers quote different rates for one Model, CANONICAL order
        // decides — not key order, and not whichever spelling this catalog uses for
        // the bare name. Checked end to end against the bundled snapshot, so both
        // the normalized merge AND the exact pass have to honour it: `gemini` and
        // `vertex_ai-language-models` disagree on 3 keys today, and every one of
        // them has a bare Vertex key that would otherwise own the row outright.
        //
        // What this does NOT catch, and why CANONICAL membership stays a human
        // check: a member reselling another lab's Model at the SAME price is
        // invisible here.
        let snapshot = include_str!("../resources/model_prices.json");
        let root: serde_json::Value = serde_json::from_str(snapshot).unwrap();
        // (provider, input, output) per normalized key.
        type Quote<'a> = (&'a str, Option<f64>, Option<f64>);
        let mut by_key: HashMap<String, Vec<Quote>> = HashMap::new();
        for (key, entry) in root.as_object().unwrap() {
            let Some(p) = entry.get("litellm_provider").and_then(|v| v.as_str()) else {
                continue;
            };
            if canonical_rank(p) == usize::MAX {
                continue;
            }
            let (i, o) = (cost(entry, "input_cost_per_token"), cost(entry, "output_cost_per_token"));
            if i.is_none() && o.is_none() {
                continue; // rebuild_prices skips these, so they can never collide
            }
            by_key.entry(normalize_model(key)).or_default().push((p, i, o));
        }

        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, snapshot, None, &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();

        let mut contested = 0;
        for (key, rows) in &by_key {
            let providers: HashSet<&str> = rows.iter().map(|r| r.0).collect();
            let prices: HashSet<(String, String)> =
                rows.iter().map(|r| (format!("{:?}", r.1), format!("{:?}", r.2))).collect();
            if providers.len() < 2 || prices.len() < 2 {
                continue; // one publisher, or one rate — nothing to settle
            }
            contested += 1;
            // Ranks are positions in CANONICAL, so two providers can never tie.
            let winner = rows.iter().min_by_key(|r| canonical_rank(r.0)).unwrap().0;
            let quoted = |f: fn(&Quote) -> Option<f64>| -> HashSet<String> {
                rows.iter().filter(|r| r.0 == winner).filter_map(f).map(|v| format!("{v:?}")).collect()
            };
            let got = rm.resolve(key).unwrap_or_else(|| panic!("`{key}` resolved to nothing"));
            // A field the winner leaves null may still be filled from a loser —
            // that is the "never lose a rate" rule — so only assert what it quotes.
            for (label, want, have) in [
                ("input", quoted(|r| r.1), got.input),
                ("output", quoted(|r| r.2), got.output),
            ] {
                if want.is_empty() {
                    continue;
                }
                assert!(
                    want.contains(&format!("{have:?}")),
                    "`{key}` resolved to {label} {have:?}, but `{winner}` is the first CANONICAL \
                     member that publishes it and quotes {want:?}. Either the tiebreak stopped \
                     working, or `{winner}` is reselling a Model it does not publish and should \
                     leave CANONICAL."
                );
            }
        }
        assert!(
            contested >= 3,
            "the snapshot no longer has publishers disagreeing on a price, so this test proves \
             nothing — confirm that is real before deleting it"
        );
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

    // A per-Model host listing, shaped like the real one: hosts ordered cheapest
    // first, the publisher NOT first, and its tag vendor matching the Model id's
    // vendor. Two z-ai endpoints so the standard/`fast` split is exercised.
    const HOSTS_GLM: &str = r#"{
      "data": {
        "id": "z-ai/glm-5.2",
        "endpoints": [
          { "provider_name": "StreamLake", "tag": "streamlake/fp8",
            "pricing": { "prompt": "0.0000007455", "completion": "0.000002343" } },
          { "provider_name": "Cloudflare", "tag": "cloudflare",
            "pricing": { "prompt": "0.0000014", "completion": "0.0000044" } },
          { "provider_name": "Z.AI", "tag": "z-ai/fp8",
            "pricing": { "prompt": "0.0000014", "completion": "0.0000044",
                         "input_cache_read": "0.00000026" } },
          { "provider_name": "Z.AI", "tag": "z-ai/fast",
            "pricing": { "prompt": "0.0000021", "completion": "0.0000066" } }
        ]
      }
    }"#;

    // Google does not host Gemini on this catalog — no endpoint tag matches.
    const HOSTS_NO_PUBLISHER: &str = r#"{
      "data": {
        "id": "google/gemini-2.5-flash",
        "endpoints": [
          { "provider_name": "DeepInfra", "tag": "deepinfra/fp8",
            "pricing": { "prompt": "0.0000003", "completion": "0.0000025" } }
        ]
      }
    }"#;

    #[test]
    fn publisher_rate_picks_the_publishers_own_endpoint() {
        let p = publisher_rate(HOSTS_GLM).unwrap();
        assert_eq!(p.model_id, "z-ai/glm-5.2");
        assert_eq!(p.publisher, "Z.AI");
                // Not StreamLake's cheaper 0.7455, and not Cloudflare's identical-looking
        // 1.4 — the publisher's own entry, which also carries a cache read rate.
        assert_eq!(p.rates.input, Some(1.4e-06));
        assert_eq!(p.rates.output, Some(4.4e-06));
        assert_eq!(p.rates.cache_read, Some(2.6e-07));
        // Of two publisher endpoints, the standard (cheaper) one is the List Price.
        assert_ne!(p.rates.input, Some(2.1e-06));
    }

    #[test]
    fn publisher_rate_is_none_when_the_publisher_does_not_host_the_model() {
        assert!(publisher_rate(HOSTS_NO_PUBLISHER).is_none());
    }

    #[test]
    fn publisher_rate_is_none_on_an_unusable_payload() {
        for bad in [
            "not json",
            r#"{"data": {}}"#,
            r#"{"data": {"id": "z-ai/glm-5.2"}}"#,
            r#"{"data": {"id": "z-ai/glm-5.2", "endpoints": []}}"#,
            // Publisher present but quotes nothing priceable.
            r#"{"data":{"id":"z-ai/x","endpoints":[{"provider_name":"Z.AI","tag":"z-ai","pricing":{"prompt":"0","completion":"0"}}]}}"#,
        ] {
            assert!(publisher_rate(bad).is_none(), "should reject: {bad}");
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
            rebuild_prices(&mut conn, fixture, None, &[]).unwrap();
            let rm = RateMap::load(&conn).unwrap();
            let r = rm.resolve("some-host/lab-model").unwrap();
            assert_eq!(r.input, 1e-06, "publisher's rate must win ({label})");
            assert_eq!(r.output, 2e-06, "publisher's rate must win ({label})");
        }
    }

    #[test]
    fn a_routed_rate_no_longer_outranks_the_primary_catalog() {
        let (_d, mut conn) = test_conn();
        let rm = tier_map(&mut conn);
        // Reverses what the superseded ADR-0003 ordering did. OpenRouter's
        // per-Model figure is a Routed Rate, not a List Price — it is blended
        // across every host and moves with their promotions — so a published
        // catalog rate outranks it even when OpenRouter matches the raw name
        // exactly and the catalog only matches after normalizing.
        let (origin, r) = rm.resolve_catalog("z-ai/glm-5.2").unwrap();
        assert_eq!(origin, "litellm");
        assert_eq!(r.input, 1.4e-06, "the catalog's rate, not the routed 0.7742");
    }

    #[test]
    fn a_publisher_rate_outranks_every_catalog() {
        let (_d, mut conn) = test_conn();
        let publisher = publisher_rate(HOSTS_GLM).unwrap();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[publisher]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Beats the Cloudflare row reached by normalization AND the routed rate,
        // and reports the publisher rather than the catalog that carried it.
        let (origin, r) = rm.resolve_catalog("z-ai/glm-5.2").unwrap();
        assert_eq!(origin, "Z.AI");
        assert_eq!(r.input, 1.4e-06);
        assert_eq!(r.cache_read, 2.6e-07);
    }

    #[test]
    fn the_primary_catalog_fills_fields_the_publisher_omits() {
        let (_d, mut conn) = test_conn();
        let publisher = publisher_rate(HOSTS_LAB_FABLE).unwrap();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[publisher]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        let (origin, r) = rm.resolve_catalog("lab-fable").unwrap();
        // The publisher's own figures win where it quotes them...
        assert_eq!(origin, "Anthropic");
        assert_eq!(r.input, 1e-05);
        assert_eq!(r.cache_write_5m, 1.25e-05);
        // ...and the one field it does not quote comes from the catalog, rather
        // than silently falling back to the 5m rate and undercounting 1h writes.
        assert_eq!(r.cache_write_1h, 2e-05);
    }

    #[test]
    fn a_model_whose_publisher_does_not_host_it_falls_to_the_catalog() {
        let (_d, mut conn) = test_conn();
        // Parsing the listing yields no publisher, so nothing reaches tier 2.
        assert!(publisher_rate(HOSTS_NO_PUBLISHER).is_none());
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        let (origin, r) = rm.resolve_catalog("priced-full").unwrap();
        assert_eq!(origin, "litellm");
        // Unchanged, rate and all — not OpenRouter's 9e-05 for the same Model.
        assert_eq!(r.input, 3e-06);
        assert_eq!(r.output, 6e-06);
    }

    /// A cached lookup that found a rate.
    fn cached_at(model_id: &str, fetched_at: i64) -> (String, PublisherLookup) {
        (model_id.to_string(), lookup_at(model_id, fetched_at, true))
    }

    /// `hosted: false` is the recorded answer "this publisher does not host it".
    fn lookup_at(model_id: &str, fetched_at: i64, hosted: bool) -> PublisherLookup {
        PublisherLookup {
            fetched_at,
            rate: hosted.then(|| PublisherRate {
                model_id: model_id.to_string(),
                publisher: "Lab".to_string(),
                rates: Row {
                    input: Some(1e-06),
                    output: Some(2e-06),
                    cache_read: None,
                    cw5m: None,
                    cw1h: None,
                },
            }),
        }
    }

    #[test]
    fn publisher_reads_skip_rates_that_are_still_fresh() {
        let now = 1_000_000;
        let targets = vec!["a/one".to_string(), "b/two".to_string(), "c/three".to_string()];
        let cached: HashMap<String, PublisherLookup> = [
            cached_at("a/one", now),                              // just read
            cached_at("b/two", now - PUBLISHER_RATE_MAX_AGE - 1),      // gone stale
        ]
        .into_iter()
        .collect();

        let reads = publisher_reads_due(&targets, &cached, now, 10);
        // Never read: fresh. Read: stale, and never-read-at-all.
        assert!(!reads.due.contains(&"a/one".to_string()));
        assert!(reads.due.contains(&"b/two".to_string()));
        assert!(reads.due.contains(&"c/three".to_string()));
        assert_eq!(reads.deferred, 0);
    }

    #[test]
    fn a_rate_exactly_at_the_window_edge_is_read() {
        let now = 1_000_000;
        let targets = vec!["a/one".to_string()];
        let cached = [cached_at("a/one", now - PUBLISHER_RATE_MAX_AGE)].into_iter().collect();
        assert_eq!(publisher_reads_due(&targets, &cached, now, 10).due.len(), 1);
    }

    #[test]
    fn publisher_reads_take_the_oldest_first_and_report_what_they_defer() {
        let now = 1_000_000;
        let stale = now - PUBLISHER_RATE_MAX_AGE - 1;
        let targets: Vec<String> =
            ["a/newest", "b/middle", "c/oldest", "d/never-read"].iter().map(|s| s.to_string()).collect();
        let cached: HashMap<String, PublisherLookup> = [
            cached_at("a/newest", stale),
            cached_at("b/middle", stale - 100),
            cached_at("c/oldest", stale - 200),
        ]
        .into_iter()
        .collect();

        // All four are due, but only two may be read this refresh.
        let reads = publisher_reads_due(&targets, &cached, now, 2);
        assert_eq!(reads.due.len(), 2);
        // Never-read comes first — it has no rate at all — then the oldest cached.
        assert_eq!(reads.due, vec!["d/never-read".to_string(), "c/oldest".to_string()]);
        // What was left is reported, so partial coverage cannot read as complete.
        assert_eq!(reads.deferred, 2);
    }

    #[test]
    fn repeated_refreshes_converge_rather_than_re_reading_the_same_models() {
        let now = 1_000_000;
        let stale = now - PUBLISHER_RATE_MAX_AGE - 1;
        let targets: Vec<String> = (0..5).map(|i| format!("lab/m{i}")).collect();
        let mut cached: HashMap<String, PublisherLookup> =
            targets.iter().enumerate().map(|(i, m)| cached_at(m, stale - i as i64)).collect();

        // Two per refresh: three refreshes must cover all five, never repeating.
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..3 {
            let reads = publisher_reads_due(&targets, &cached, now, 2);
            for id in &reads.due {
                cached.insert(id.clone(), cached_at(id, now).1); // reading refreshes it
                seen.push(id.clone());
            }
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5, "every target read exactly once across refreshes");
        assert_eq!(publisher_reads_due(&targets, &cached, now, 2).deferred, 0);
    }

    #[test]
    fn a_model_with_no_publisher_does_not_starve_the_others() {
        let now = 1_000_000;
        let stale = now - PUBLISHER_RATE_MAX_AGE - 1;
        let targets: Vec<String> =
            ["a/hosted-stale", "b/not-hosted"].iter().map(|s| s.to_string()).collect();

        // "b" was asked and the answer was a definitive no — its publisher does
        // not host it. That answer is recorded, so it ages like any other and does
        // NOT come back every refresh ahead of everything else. Without this, a
        // Ledger with enough such Models spends its whole budget re-asking them
        // and never re-reads a real rate again.
        let mut cached: HashMap<String, PublisherLookup> = HashMap::new();
        cached.insert("a/hosted-stale".into(), lookup_at("a/hosted-stale", stale, true));
        cached.insert("b/not-hosted".into(), lookup_at("b/not-hosted", now, false));

        let reads = publisher_reads_due(&targets, &cached, now, 1);
        assert_eq!(reads.due, vec!["a/hosted-stale".to_string()]);
        assert_eq!(reads.deferred, 0, "the un-hosted Model is fresh, not merely skipped");
    }

    #[test]
    fn a_no_publisher_answer_is_recorded_so_it_is_not_re_asked() {
        let mut cached = HashMap::new();
        // The host listing WAS read; there was simply no publisher in it.
        record_lookups(&mut cached, vec![("c/not-hosted".to_string(), None)], 500);
        assert_eq!(cached["c/not-hosted"].fetched_at, 500);
        assert!(cached["c/not-hosted"].rate.is_none());

        // Being an answer, it is fresh — so the next refresh does not ask again,
        // which is what keeps it from starving Models that do have a rate.
        let targets = vec!["c/not-hosted".to_string()];
        assert!(publisher_reads_due(&targets, &cached, 500, 10).due.is_empty());
        // ...and it does come back round once the window has passed.
        assert_eq!(
            publisher_reads_due(&targets, &cached, 500 + PUBLISHER_RATE_MAX_AGE, 10).due.len(),
            1
        );
    }

    #[test]
    fn never_cached_models_keep_a_stable_order() {
        // Several never-read Models share the same sort key, so ordering falls
        // through to target order. Pin it: without a stable rule, two refreshes
        // could pick different subsets and neither would converge.
        let targets: Vec<String> = ["a/one", "b/two", "c/three"].iter().map(|s| s.to_string()).collect();
        let empty = HashMap::new();
        let first = publisher_reads_due(&targets, &empty, 0, 2);
        let again = publisher_reads_due(&targets, &empty, 0, 2);
        assert_eq!(first.due, vec!["a/one".to_string(), "b/two".to_string()]);
        assert_eq!(first.due, again.due);
    }

    #[test]
    fn publisher_reads_are_empty_without_targets() {
        let reads = publisher_reads_due(&[], &HashMap::new(), 0, 10);
        assert!(reads.due.is_empty());
        assert_eq!(reads.deferred, 0);
    }

    #[test]
    fn a_model_not_read_this_refresh_keeps_the_rate_it_already_had() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // Both cached moments ago, so both are inside the staleness window and
        // this refresh reads nothing — which also keeps the test off the network.
        // It must still return both: bounding the work must never cost a Model
        // its List Price and drop it to a catalog rate.
        let cached: HashMap<String, PublisherLookup> = [
            cached_at("a/one", now),
            cached_at("b/two", now),
            // A recorded "no publisher hosts this" must survive the round-trip
            // through the cache file, or it would read as never-asked and be
            // re-requested on every refresh.
            ("c/not-hosted".to_string(), lookup_at("c/not-hosted", now, false)),
        ]
        .into_iter()
        .collect();
        std::fs::write(
            dir.path().join("openrouter_publishers.json"),
            serde_json::to_string(&cached).unwrap(),
        )
        .unwrap();

        let ids =
            vec!["a/one".to_string(), "b/two".to_string(), "c/not-hosted".to_string()];
        let got = load_publisher_rates(dir.path(), &ids);
        assert_eq!(got.len(), 2, "both cached rates survive a refresh that read neither");
        assert!(got.iter().all(|r| r.rates.input == Some(1e-06)));
        // The negative yields no rate, so that Model falls to the catalogs.
        assert!(!got.iter().any(|r| r.model_id == "c/not-hosted"));

        // And it is still recorded afterwards, not dropped on rewrite.
        let after: HashMap<String, PublisherLookup> = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("openrouter_publishers.json")).unwrap(),
        )
        .unwrap();
        assert!(after["c/not-hosted"].rate.is_none());
    }

    #[test]
    fn load_publisher_rates_does_nothing_without_targets() {
        // Guards the chunking arithmetic: an empty target list must not divide by
        // zero, spawn a lane, or touch the network.
        let dir = tempfile::tempdir().unwrap();
        assert!(load_publisher_rates(dir.path(), &[]).is_empty());
    }

    #[test]
    fn a_model_with_no_published_rate_still_gets_the_routed_rate() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // No catalog coverage at all: the Routed Rate is weaker than a List Price
        // but better than Unpriced, so it survives as the last tier.
        let (origin, r) = rm.resolve_catalog("claude-opus-5").unwrap();
        assert_eq!(origin, "openrouter");
        assert_eq!(r.input, 5e-06);
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
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[]).unwrap();
        set_override(
            &conn,
            "z-ai/glm-5.2",
            OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: None },
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("z-ai/glm-5.2").unwrap().input, 9e-06);
        // resolve_catalog still reports the catalog match, ignoring the Override.
        assert_eq!(rm.resolve_catalog("z-ai/glm-5.2").unwrap().0, "litellm");
    }

    #[test]
    fn an_override_outranks_a_publisher_rate_too() {
        let (_d, mut conn) = test_conn();
        let publisher = publisher_rate(HOSTS_GLM).unwrap();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[publisher]).unwrap();
        set_override(
            &conn,
            "z-ai/glm-5.2",
            OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: None },
        )
        .unwrap();
        let rm = RateMap::load(&conn).unwrap();
        assert_eq!(rm.resolve("z-ai/glm-5.2").unwrap().input, 9e-06);
        assert_eq!(rm.resolve_catalog("z-ai/glm-5.2").unwrap().0, "Z.AI");
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
        rebuild_prices(&mut conn, TIER_LITELLM, None, &[]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
        // Exactly today's behaviour: the normalized Cloudflare row is the only match.
        let (origin, r) = rm.resolve_catalog("z-ai/glm-5.2").unwrap();
        assert_eq!(origin, "litellm");
        assert_eq!(r.input, 1.4e-06);
        assert_eq!(rm.resolve_catalog("claude-opus-5"), None);
    }

    #[test]
    fn a_publisher_rate_without_cache_writes_is_still_cache_estimated() {
        let (_d, mut conn) = test_conn();
        // The publisher quotes cache READS but no cache write, and the primary
        // catalog has no cache rates to fill the gap -> Cache-Estimated survives
        // the merge rather than being masked by it.
        let publisher = publisher_rate(HOSTS_GLM).unwrap();
        rebuild_prices(&mut conn, TIER_LITELLM, Some(TIER_OPENROUTER), &[publisher]).unwrap();
        let rm = RateMap::load(&conn).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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
    fn publisher_targets_are_only_ledger_models_the_catalog_serves() {
        let (_d, conn) = test_conn();
        seed_event(&conn, "z-ai/glm-5.2", "hermes"); // matches an id exactly
        seed_event(&conn, "claude-opus-5", "claude"); // matches after normalizing
        seed_event(&conn, "qwen3.6-35b-mtp", "hermes"); // served by nobody
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, source_file) \
             VALUES ('pi:tool:1', 'pi', 0, NULL, 'pi.jsonl')",
            [],
        )
        .unwrap();

        let targets = ledger_publisher_targets(&conn, TIER_OPENROUTER).unwrap();
        // One request per Model, and only for Models we actually have.
        assert_eq!(targets, vec!["anthropic/claude-opus-5", "z-ai/glm-5.2"]);
        // openai/priced-full is served but absent from the Ledger, so it is never
        // fetched; Unattributed Usage has no Model and contributes nothing.
        assert!(!targets.iter().any(|t| t.contains("priced-full")));
    }

    #[test]
    fn publisher_targets_are_empty_for_an_empty_ledger() {
        let (_d, conn) = test_conn();
        assert!(ledger_publisher_targets(&conn, TIER_OPENROUTER).unwrap().is_empty());
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
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
        assert!(models_needing_lookup(&conn, &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn override_set_delete_roundtrip_via_model_pricing() {
        let (_d, mut conn) = test_conn();
        rebuild_prices(&mut conn, MP_FIXTURE, None, &[]).unwrap();
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
