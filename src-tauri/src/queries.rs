use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct Filters {
    pub tools: Vec<String>,
    pub models: Vec<String>,
    pub project: Option<String>,
    #[ts(optional, type = "number")]
    pub start_ts: Option<i64>,
    #[ts(optional, type = "number")]
    pub end_ts: Option<i64>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct Summary {
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read_tokens: i64,
    #[ts(type = "number")]
    pub cache_write_tokens: i64,
    #[ts(type = "number")]
    pub total_tokens: i64,
    #[ts(type = "number")]
    pub requests: i64,
    pub cost: Option<f64>,
    pub has_unpriced: bool,
    #[ts(type = "number")]
    pub unattributed_tokens: i64,
    pub unpriced_models: Vec<String>,
    pub cache_estimated_models: Vec<String>,
    pub cache_hit_rate: f64,
    /// Distinct Sessions in the window. Counted here rather than summed from
    /// per-day counts: a Session that spans days would be counted once per day
    /// it touches.
    #[ts(type = "number")]
    pub convs: i64,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct BreakdownRow {
    // None is reserved for a model-breakdown row whose Usage has no Model.
    // Project/tool breakdowns continue to return named keys.
    pub key: Option<String>,
    pub source: Option<String>,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read_tokens: i64,
    #[ts(type = "number")]
    pub cache_write_tokens: i64,
    #[ts(type = "number")]
    pub total_tokens: i64,
    #[ts(type = "number")]
    pub requests: i64,
    pub cost: Option<f64>,
    #[ts(type = "number | null")]
    pub reasoning_tokens: Option<i64>,
    #[ts(type = "number")]
    pub convs: i64,
    pub cache_estimated: bool,
    // True when any of the row's Models is Unpriced — a Some(cost) is then a
    // Partial Cost (glossary: shown with "≥", never as a complete total).
    pub has_unpriced: bool,
    // Tokens in this row that have no Model. Kept outside Model identity so
    // future adapters never need a sentinel model name.
    #[ts(type = "number")]
    pub unattributed_tokens: i64,
}

use std::collections::{BTreeMap, HashMap};
use rusqlite::{params_from_iter, types::Value, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::limits_estimator::{coheres, recency_horizon, Candidate};
use crate::limits_evidence::{self, ReasonCode, SeriesKey};
use crate::limits_readiness::{self, Evaluation, ReadinessState};
use crate::pricing::RateMap;
use crate::types::LimitReading;

// Builds the dynamic WHERE fragment (empty vec = no constraint; end_ts exclusive).
fn build_where(f: &Filters) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if !f.tools.is_empty() {
        let ph = f.tools.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        clauses.push(format!("source IN ({ph})"));
        for t in &f.tools {
            params.push(Value::Text(t.clone()));
        }
    }
    if !f.models.is_empty() {
        let ph = f.models.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        clauses.push(format!("model IN ({ph})"));
        for m in &f.models {
            params.push(Value::Text(m.clone()));
        }
    }
    if let Some(p) = &f.project {
        clauses.push("project = ?".to_string());
        params.push(Value::Text(p.clone()));
    }
    if let Some(s) = f.start_ts {
        clauses.push("timestamp >= ?".to_string());
        params.push(Value::Integer(s));
    }
    if let Some(e) = f.end_ts {
        clauses.push("timestamp < ?".to_string());
        params.push(Value::Integer(e));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (where_sql, params)
}

#[derive(Default)]
struct CostContribution {
    cost: f64,
    priced_tokens: i64,
    unattributed_tokens: i64,
    unpriced: bool,
    cache_estimated: bool,
}

fn cost_contribution(
    rates: &RateMap,
    model: Option<&str>,
    day: &str,
    tokens: [i64; 5],
) -> CostContribution {
    let [input, output, cache_read, cache_write_5m, cache_write_1h] = tokens;
    let tokens = input + output + cache_read + cache_write_5m + cache_write_1h;
    let Some(model) = model else {
        return CostContribution { unattributed_tokens: tokens, ..Default::default() };
    };
    let Some(model_rates) = rates.resolve_at(model, day) else {
        return CostContribution { unpriced: tokens > 0, ..Default::default() };
    };
    CostContribution {
        cost: model_rates.cost(input, output, cache_read, cache_write_5m, cache_write_1h),
        priced_tokens: tokens,
        cache_estimated: model_rates.cache_gap(cache_read, cache_write_5m, cache_write_1h),
        ..Default::default()
    }
}

pub fn summary(conn: &Connection, f: &Filters) -> rusqlite::Result<Summary> {
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    // Two cheap passes, deliberately: folding the Session count into this scan
    // (GROUP BY model, source, session) was measured SLOWER on a real Ledger —
    // grouping into 37 model buckets plus a separate COUNT DISTINCT beats one
    // scan paying a fat string key per row. breakdown() reaches the opposite
    // verdict because its group key is already wide; see that comment.
    let sql = format!(
        "SELECT tokenledger_local_bucket(timestamp, 0) AS price_day, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls) \
         FROM events {where_sql} GROUP BY price_day, model"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
        ))
    })?;

    let (mut input, mut output, mut cache_read, mut cw5m, mut cw1h, mut requests) =
        (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut cost = 0.0f64;
    let mut priced_tokens = 0i64;
    let mut unattributed_tokens = 0i64;
    let mut unpriced_models: Vec<String> = Vec::new();
    let mut cache_estimated_models: Vec<String> = Vec::new();

    for row in rows {
        let (day, model, in_, out, cr, w5, w1, calls) = row?;
        input += in_;
        output += out;
        cache_read += cr;
        cw5m += w5;
        cw1h += w1;
        requests += calls;
        let contribution = cost_contribution(
            &rates,
            model.as_deref(),
            &day,
            [in_, out, cr, w5, w1],
        );
        cost += contribution.cost;
        priced_tokens += contribution.priced_tokens;
        unattributed_tokens += contribution.unattributed_tokens;
        if let Some(model) = model {
            if contribution.unpriced {
                unpriced_models.push(model);
            } else if contribution.cache_estimated {
                cache_estimated_models.push(model);
            }
        }
    }
    unpriced_models.sort();
    unpriced_models.dedup();
    cache_estimated_models.sort();
    cache_estimated_models.dedup();

    // Distinct Sessions over the whole window. `source || ':' || session_id`
    // keeps two Sources that happen to reuse an id apart, and SQLite's
    // NULL-propagating concat drops rows with no session identity — the same
    // "NULL session ids count zero distinct" rule the per-bucket count follows.
    let convs: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT source || ':' || session_id) FROM events {where_sql}"
        ),
        params_from_iter(params.iter()),
        |r| r.get(0),
    )?;

    let cache_write = cw5m + cw1h;
    let total = input + output + cache_read + cache_write;
    let denom = input + cache_read + cache_write;
    let cache_hit_rate = if denom > 0 {
        cache_read as f64 / denom as f64
    } else {
        0.0
    };

    Ok(Summary {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        total_tokens: total,
        requests,
        // None is Unpriced: usage whose Cost this window cannot compute. A
        // window holding no usage at all has no such gap — it cost zero, and
        // every surface reads that out as $0.00 rather than "unpriced".
        cost: if priced_tokens > 0 || total == 0 { Some(cost) } else { None },
        has_unpriced: !unpriced_models.is_empty(),
        unattributed_tokens,
        unpriced_models,
        cache_estimated_models,
        cache_hit_rate,
        convs,
    })
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct SeriesPoint {
    pub bucket: String,
    pub source: String,
    #[ts(type = "Record<string, number>")]
    pub by_model: HashMap<String, i64>, // model -> total tokens within (bucket, source)
    // Usage with no Model stays beside, never inside, the Model map.
    #[ts(type = "number")]
    pub unattributed_tokens: i64,
    pub has_unpriced: bool,
    /// Cache-Estimated (CONTEXT.md): this bucket counted cache tokens whose
    /// rate is absent. Same predicate Cost uses (`Rates::cache_gap`).
    pub cache_estimated: bool,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read_tokens: i64,
    #[ts(type = "number")]
    pub cache_write_tokens: i64,
    #[ts(type = "number")]
    pub total_tokens: i64,
    #[ts(type = "number | null")]
    pub reasoning_tokens: Option<i64>,
    pub cost: f64,
    #[ts(type = "number")]
    pub requests: i64,
    #[ts(type = "number")]
    pub convs: i64,
}

// Merges a nullable per-group SUM into an accumulator: only Some contributes,
// so a group whose values are all NULL stays None (never coerced to 0).
fn add_opt(acc: &mut Option<i64>, v: Option<i64>) {
    if let Some(x) = v {
        *acc = Some(acc.unwrap_or(0) + x);
    }
}

fn hourly_flag(bucket: &str) -> i32 {
    i32::from(bucket == "hour")
}

fn price_day(bucket: &str) -> &str {
    bucket.get(..10).unwrap_or(bucket)
}

// Per-(bucket, source) series — the real-data twin of the frontend mock's DAYS.
pub fn series(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<SeriesPoint>> {
    let hourly = hourly_flag(bucket);
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);

    // Tokens/cost need per-model rows for rate resolution.
    let sql = format!(
        "SELECT tokenledger_local_bucket(timestamp, {hourly}) AS bucket, source, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens) \
         FROM events {where_sql} GROUP BY bucket, source, model ORDER BY bucket, source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?,
            r.get::<_, Option<i64>>(9)?,
        ))
    })?;

    let mut idx: HashMap<(String, String), usize> = HashMap::new();
    let mut points: Vec<SeriesPoint> = Vec::new();
    for row in rows {
        let (bucket, source, model, in_, out, cr, w5, w1, calls, reasoning) = row?;
        let tokens = in_ + out + cr + w5 + w1;
        let contribution = cost_contribution(
            &rates,
            model.as_deref(),
            price_day(&bucket),
            [in_, out, cr, w5, w1],
        );
        // Clone into the map key; avoid moving (bucket, source) before push.
        let i = *idx.entry((bucket.clone(), source.clone())).or_insert_with(|| {
            points.push(SeriesPoint {
                bucket: bucket.clone(),
                source: source.clone(),
                by_model: HashMap::new(),
                unattributed_tokens: 0,
                has_unpriced: false,
                cache_estimated: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                total_tokens: 0,
                reasoning_tokens: None,
                cost: 0.0,
                requests: 0,
                convs: 0,
            });
            points.len() - 1
        });
        let p = &mut points[i];
        if let Some(model) = model {
            *p.by_model.entry(model).or_insert(0) += tokens;
        }
        p.unattributed_tokens += contribution.unattributed_tokens;
        p.has_unpriced |= contribution.unpriced;
        p.cache_estimated |= contribution.cache_estimated;
        p.input_tokens += in_;
        p.output_tokens += out;
        p.cache_read_tokens += cr;
        p.cache_write_tokens += w5 + w1;
        p.total_tokens += in_ + out + cr + w5 + w1;
        p.requests += calls;
        p.cost += contribution.cost;
        add_opt(&mut p.reasoning_tokens, reasoning);
    }

    // Convs need distinct-count at (bucket, source) — a session can span
    // models, so distinct-per-model counts cannot be summed.
    let sql2 = format!(
        "SELECT tokenledger_local_bucket(timestamp, {hourly}) AS bucket, source, \
         COUNT(DISTINCT session_id) FROM events {where_sql} GROUP BY bucket, source"
    );
    let mut stmt2 = conn.prepare(&sql2)?;
    let crows = stmt2.query_map(params_from_iter(params.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in crows {
        let (bucket, source, convs) = row?;
        if let Some(&i) = idx.get(&(bucket, source)) {
            points[i].convs = convs;
        }
    }
    Ok(points)
}

#[derive(Default)]
struct Agg {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total: i64,
    requests: i64,
    cost: f64,
    priced: i64,
    reasoning: Option<i64>,
    convs: i64,
    cache_estimated: bool,
    unpriced: bool,
    unattributed: i64,
}

pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>> {
    let group_col = match by {
        "tool" => "source",
        "project" => "project",
        _ => "model",
    };
    // Model rows additionally split by source so the UI can scope models to a
    // tool; a constant NULL leaves other modes' grouping untouched.
    let src_expr = if group_col == "model" { "source" } else { "NULL" };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    // One walk of the window at (grp, src, model, session) grain: the per-group
    // distinct-Session count used to be a second full scan of the same rows.
    // Measured ~25-35% faster on a real Ledger's month/total windows — this
    // query's group key is already wide, so adding session costs little next to
    // the scan it saves (summary() measures the other way; see its comment).
    // The finer rows re-aggregate to (grp, src, model) grain below — integer
    // sums are exact, so the pricing loop sees the same per-model figures the
    // GROUP BY grp, src, model query produced.
    let sql = format!(
        "SELECT {group_col} AS grp, {src_expr} AS src, model, session_id, \
         tokenledger_local_bucket(timestamp, 0) AS price_day, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens) \
         FROM events {where_sql} GROUP BY grp, src, model, session_id, price_day"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?, r.get::<_, i64>(9)?, r.get::<_, i64>(10)?,
            r.get::<_, Option<i64>>(11)?,
        ))
    })?;

    let group_key = |group: Option<String>| {
        if group_col == "model" {
            group
        } else {
            Some(group.unwrap_or_else(|| "unknown".to_string()))
        }
    };
    // BTreeMap so each (grp, src) group's pricing runs over its models in the
    // same sorted order the old GROUP BY emitted them.
    type Key4 = (Option<String>, Option<String>, Option<String>, String);
    let mut subs: std::collections::BTreeMap<Key4, ([i64; 6], Option<i64>)> = Default::default();
    // Convs at the row's own grain (distinct sessions can span models); a row
    // with no session identity counts zero distinct, like the old
    // COUNT(DISTINCT session_id) pass.
    let mut sessions: std::collections::HashSet<(Option<String>, Option<String>, String)> =
        Default::default();
    for row in rows {
        let (grp, src, model, session, day, in_, out, cr, w5, w1, calls, reasoning) = row?;
        let grp = group_key(grp);
        if let Some(session) = session {
            sessions.insert((grp.clone(), src.clone(), session));
        }
        let (sums, r_acc) = subs.entry((grp, src, model, day)).or_default();
        sums[0] += in_;
        sums[1] += out;
        sums[2] += cr;
        sums[3] += w5;
        sums[4] += w1;
        sums[5] += calls;
        if let Some(r) = reasoning {
            *r_acc = Some(r_acc.unwrap_or(0) + r);
        }
    }

    let mut map: HashMap<(Option<String>, Option<String>), Agg> = HashMap::new();
    for ((grp, src, model, day), ([in_, out, cr, w5, w1, calls], reasoning)) in subs {
        let a = map.entry((grp, src)).or_default();
        a.input += in_;
        a.output += out;
        a.cache_read += cr;
        a.cache_write += w5 + w1;
        a.total += in_ + out + cr + w5 + w1;
        a.requests += calls;
        if let Some(r) = reasoning {
            a.reasoning = Some(a.reasoning.unwrap_or(0) + r);
        }
        let contribution = cost_contribution(
            &rates,
            model.as_deref(),
            &day,
            [in_, out, cr, w5, w1],
        );
        a.cost += contribution.cost;
        a.priced += contribution.priced_tokens;
        a.unpriced |= contribution.unpriced;
        a.cache_estimated |= contribution.cache_estimated;
        a.unattributed += contribution.unattributed_tokens;
    }
    for (grp, src, _) in sessions {
        if let Some(a) = map.get_mut(&(grp, src)) {
            a.convs += 1;
        }
    }

    let mut out: Vec<BreakdownRow> = map
        .into_iter()
        .map(|((key, source), a)| BreakdownRow {
            key,
            source,
            input_tokens: a.input,
            output_tokens: a.output,
            cache_read_tokens: a.cache_read,
            cache_write_tokens: a.cache_write,
            total_tokens: a.total,
            requests: a.requests,
            cost: if a.priced > 0 { Some(a.cost) } else { None },
            reasoning_tokens: a.reasoning,
            convs: a.convs,
            cache_estimated: a.cache_estimated,
            has_unpriced: a.unpriced,
            unattributed_tokens: a.unattributed,
        })
        .collect();
    out.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    Ok(out)
}

/// One date window of the Ledger's priced facts: Summary plus the three
/// breakdowns. Source selection is a presentation filter, not a query filter.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LedgerWindow {
    pub summary: Summary,
    pub models: Vec<BreakdownRow>,
    pub projects: Vec<BreakdownRow>,
    /// Breakdown by Source (`breakdown(..., "tool")`).
    pub sources: Vec<BreakdownRow>,
}

/// The four priced reads as one snapshot, so a Scan cannot commit between
/// Summary and a breakdown — the same reason `limits` holds an unchecked
/// transaction.
pub fn window(conn: &Connection, f: &Filters) -> rusqlite::Result<LedgerWindow> {
    let read = conn.unchecked_transaction()?;
    let out = LedgerWindow {
        summary: summary(conn, f)?,
        models: breakdown(conn, "model", f)?,
        projects: breakdown(conn, "project", f)?,
        sources: breakdown(conn, "tool", f)?,
    };
    read.finish()?;
    Ok(out)
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxResource {
    pub source: String,
    pub kind: String,
    pub name: String,
}

// Day-granular WHERE for the ctx_* tables (deduped/aggregated per local day):
// optional source IN plus ts bounds mapped to local-day strings — end_ts
// exclusive → day of end_ts − 1s inclusive. Empty when unconstrained.
fn day_where(f: &Filters) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if !f.tools.is_empty() {
        let ph = f.tools.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        clauses.push(format!("source IN ({ph})"));
        for t in &f.tools {
            params.push(Value::Text(t.clone()));
        }
    }
    if let Some(s) = f.start_ts {
        clauses.push("day >= strftime('%Y-%m-%d', ?, 'unixepoch', 'localtime')".to_string());
        params.push(Value::Integer(s));
    }
    if let Some(e) = f.end_ts {
        clauses.push("day <= strftime('%Y-%m-%d', ?, 'unixepoch', 'localtime')".to_string());
        params.push(Value::Integer(e - 1));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (where_sql, params)
}

// Distinct resource names (skills / MCP servers / agents / memory files) seen
// in range, per source — the Context Breakdown meta line and skill drill-down.
pub(crate) fn ctx_resources(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxResource>> {
    let (where_sql, params) = day_where(f);
    let sql = format!(
        "SELECT DISTINCT source, kind, name FROM ctx_resources {where_sql} \
         ORDER BY source, kind, name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxResource { source: r.get(0)?, kind: r.get(1)?, name: r.get(2)? })
    })?;
    rows.collect()
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxBuckets {
    pub source: String,
    #[ts(type = "number")]
    pub history: i64,          // cache_read + non-first cache writes
    #[ts(type = "number")]
    pub new_input: i64,        // fresh input_tokens
    #[ts(type = "number | null")]
    pub system: Option<i64>,   // first cache write per session; NULL when unknowable
    #[ts(type = "number")]
    pub response: i64,         // max(0, output − reasoning)
    #[ts(type = "number | null")]
    pub reasoning: Option<i64>,
}

// Exact usage-field buckets (spec 2026-07-10-context-drilldown). The window
// runs over the WHOLE table so a session straddling the range still knows
// which cache-write was its first; range/tool/model/project filters apply
// OUTSIDE the window. A first-cw event outside the range means in-range
// writes count as history — conservative, never inflates System.
pub(crate) fn ctx_buckets(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxBuckets>> {
    // A Hermes Usage Record is Session-granularity — one Record stands for a
    // whole Session of calls — so "first cache-write per session = System
    // prompt" cannot apply to it; its writes count as history, not System.
    //
    // The session's first cache write is found over the WHOLE Ledger, never the
    // filtered window: a session that started before the window has its System
    // write outside it, so its in-window writes are all history. first_ts and
    // session_first_cw walk idx_events_first_cw (a few hundred sessions) instead
    // of ranking the whole events table per call, which made each range switch
    // pay a full-table sort. session_first_cw picks the (timestamp, dedup_key)
    // minimum per session: MIN(timestamp) first, then MIN(dedup_key) among the
    // Usage Records sharing that timestamp.
    //
    // One name for the cache-write sum. In the two CTE WHEREs it must render
    // verbatim as idx_events_first_cw's predicate writes it (db.rs SCHEMA_V13),
    // or the planner cannot prove the partial index applies and the full scan
    // comes back — so this const is shared with the SUM arms, never reformatted.
    const CW: &str = "cache_write_5m_tokens + cache_write_1h_tokens";
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "WITH first_ts AS ( \
           SELECT source, session_id, MIN(timestamp) AS ts FROM events \
           WHERE {CW} > 0 \
             AND session_id IS NOT NULL AND source != 'hermes' \
           GROUP BY source, session_id), \
         session_first_cw AS ( \
           SELECT e.source AS sfc_source, e.session_id AS sfc_session, \
                  e.timestamp AS sfc_ts, MIN(e.dedup_key) AS sfc_dedup_key \
           FROM events e JOIN first_ts m \
             ON e.source = m.source AND e.session_id = m.session_id AND e.timestamp = m.ts \
           WHERE {CW} > 0 \
             AND e.session_id IS NOT NULL \
           GROUP BY e.source, e.session_id, e.timestamp) \
         SELECT source, \
           SUM(cache_read_tokens) + SUM(CASE WHEN {CW} > 0 AND sfc_dedup_key IS NULL THEN {CW} ELSE 0 END), \
           SUM(input_tokens), \
           SUM(CASE WHEN sfc_dedup_key IS NOT NULL THEN {CW} END), \
           SUM(output_tokens), \
           SUM(reasoning_tokens) \
         FROM events LEFT JOIN session_first_cw \
           ON sfc_source = source AND sfc_session = session_id \
           AND sfc_ts = timestamp AND sfc_dedup_key = dedup_key \
         {where_sql} GROUP BY source ORDER BY source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
            r.get::<_, Option<i64>>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<i64>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (source, history, new_input, system, output, reasoning) = row?;
        out.push(CtxBuckets {
            source,
            history,
            new_input,
            system,
            response: (output - reasoning.unwrap_or(0)).max(0),
            reasoning,
        });
    }
    Ok(out)
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxToolRow {
    pub source: String,
    pub name: String,
    #[ts(type = "number")]
    pub est_tokens: i64,
    #[ts(type = "number")]
    pub calls: i64,
}

// Per-tool weights in range. Ignores model/project.
pub(crate) fn ctx_tools(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxToolRow>> {
    let (where_sql, params) = day_where(f);
    let sql = format!(
        "SELECT source, name, SUM(est_tokens), SUM(calls) FROM ctx_tools {where_sql} \
         GROUP BY source, name ORDER BY SUM(est_tokens) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxToolRow { source: r.get(0)?, name: r.get(1)?, est_tokens: r.get(2)?, calls: r.get(3)? })
    })?;
    rows.collect()
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxSkillRow {
    pub source: String,
    pub name: String,
    #[ts(type = "number")]
    pub est_tokens: i64,
    /// Injections, not distinct skills: a re-invoked skill re-loads its whole
    /// body, so this is what makes est_tokens grow.
    #[ts(type = "number")]
    pub uses: i64,
}

// Per-skill weights in range, heaviest first. Ignores model/project, like
// ctx_tools — these are context composition, not billed usage.
pub(crate) fn ctx_skills(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxSkillRow>> {
    let (where_sql, params) = day_where(f);
    let sql = format!(
        "SELECT source, name, SUM(est_tokens), SUM(uses) FROM ctx_skills_usage {where_sql} \
         GROUP BY source, name ORDER BY SUM(est_tokens) DESC, name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxSkillRow { source: r.get(0)?, name: r.get(1)?, est_tokens: r.get(2)?, uses: r.get(3)? })
    })?;
    rows.collect()
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxExecRow {
    pub source: String,
    pub kind: String,
    pub exe: String,
    pub cmd: String,
    #[ts(type = "number")]
    pub est_tokens: i64,
    #[ts(type = "number")]
    pub calls: i64,
}

// Bash command facets in range. Ignores model/project (table has neither).
// `source` groups by producer but is claude-only by design: codex logs shell
// commands as JSON arrays inside function_call payloads (no shell string for
// exec_class), and the Overview renders exec facets only under the Bash node.
pub(crate) fn ctx_exec(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxExecRow>> {
    let (where_sql, params) = day_where(f);
    let sql = format!(
        "SELECT source, kind, exe, cmd, SUM(est_tokens), SUM(calls) FROM ctx_exec {where_sql} \
         GROUP BY source, kind, exe, cmd ORDER BY SUM(est_tokens) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxExecRow {
            source: r.get(0)?, kind: r.get(1)?, exe: r.get(2)?, cmd: r.get(3)?,
            est_tokens: r.get(4)?, calls: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// Per-Source billed Context for a window. `messages`/`system`/`reasoning` and
/// the estimated categories are NULL when the Source cannot attribute them —
/// the same "—" vs zero rule the card uses, so series is not a second home.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxSourceTotals {
    pub source: String,
    #[ts(type = "number")]
    pub billed: i64,
    #[ts(type = "number")]
    pub reused: i64,
    #[ts(type = "number | null")]
    pub messages: Option<i64>,
    #[ts(type = "number | null")]
    pub system: Option<i64>,
    #[ts(type = "number | null")]
    pub reasoning: Option<i64>,
    #[ts(type = "number | null")]
    pub toolcalls: Option<i64>,
    #[ts(type = "number | null")]
    pub agents: Option<i64>,
    #[ts(type = "number | null")]
    pub mcp: Option<i64>,
    #[ts(type = "number | null")]
    pub skills: Option<i64>,
}

fn ctx_source_totals(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxSourceTotals>> {
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT source, \
         SUM(input_tokens) + SUM(cache_read_tokens) + SUM(cache_write_5m_tokens) + SUM(cache_write_1h_tokens), \
         SUM(cache_read_tokens), \
         SUM(ctx_messages), SUM(ctx_system), SUM(ctx_reasoning), \
         SUM(ctx_toolcalls), SUM(ctx_agents), SUM(ctx_mcp), SUM(ctx_skills) \
         FROM events {where_sql} GROUP BY source ORDER BY source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxSourceTotals {
            source: r.get(0)?,
            billed: r.get(1)?,
            reused: r.get(2)?,
            messages: r.get(3)?,
            system: r.get(4)?,
            reasoning: r.get(5)?,
            toolcalls: r.get(6)?,
            agents: r.get(7)?,
            mcp: r.get(8)?,
            skills: r.get(9)?,
        })
    })?;
    rows.collect()
}

/// One date window of the Ledger's Context. Source selection is a
/// presentation filter, not a query filter — the report stacks every
/// reporting Source.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LedgerContext {
    pub resources: Vec<CtxResource>,
    pub buckets: Vec<CtxBuckets>,
    pub tools: Vec<CtxToolRow>,
    pub skills: Vec<CtxSkillRow>,
    pub exec: Vec<CtxExecRow>,
    pub totals: Vec<CtxSourceTotals>,
}

/// Context for a date window as one snapshot, so a Scan cannot commit between
/// the exact buckets, the weight tables, and the billed totals — the same
/// reason `window` holds an unchecked transaction.
pub fn context(conn: &Connection, f: &Filters) -> rusqlite::Result<LedgerContext> {
    let read = conn.unchecked_transaction()?;
    let out = LedgerContext {
        resources: ctx_resources(conn, f)?,
        buckets: ctx_buckets(conn, f)?,
        tools: ctx_tools(conn, f)?,
        skills: ctx_skills(conn, f)?,
        exec: ctx_exec(conn, f)?,
        totals: ctx_source_totals(conn, f)?,
    };
    read.finish()?;
    Ok(out)
}

#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LimitWindow {
    /// Opaque (never parsed for structure): Claude's own response key, Codex's
    /// `w{canonical minutes}`.
    pub window_key: String,
    /// Absent where the vendor never named the window's length: the card then
    /// draws a bar with no time tick rather than inventing an axis.
    #[ts(type = "number | null")]
    pub window_minutes: Option<i64>,
    /// The vendor's own figure, unconverted.
    pub used_pct: f64,
    #[ts(type = "number")]
    pub resets_at: i64,
    #[ts(type = "number")]
    pub observed_at: i64,
    /// Exactly one tagged evaluation, sharing this query's single
    /// `evaluatedAt` with every other window in the response.
    pub estimate: LimitEstimateEvaluation,
}

/// One completed epoch the policy weighed. Compact by design: the exact
/// contributing Readings and Usage Records stay reconstructible from the Series
/// and this stretch, and are never sent on a page load.
#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct EstimateEpochSummary {
    /// A privacy-safe diagnostic identity: a digest of the Series and the epoch,
    /// so two epochs can be told apart and neither can be read back into an
    /// account.
    pub epoch_key: String,
    #[ts(type = "number")]
    pub ended_at: i64,
    #[ts(type = "number")]
    pub movement_points: i64,
    #[ts(type = "number")]
    pub positive_movements: usize,
    /// Stable-core membership — the count the row reports, not every candidate.
    pub in_core: bool,
    /// Why this epoch sits outside the core, where it does and a core exists:
    /// adding it back would break the endpoint-rounding intersection or the
    /// ratio spread, and the code names which. Empty for core members — and for
    /// every candidate when no core formed at all, where no single epoch can be
    /// blamed for the set.
    pub reason_codes: Vec<ReasonCode>,
}

/// One reason, and how often it applied.
#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct EstimateRejection {
    pub reason_code: ReasonCode,
    #[ts(type = "number")]
    pub count: usize,
}

/// The narrowest and widest ratio in the set the answer came from.
#[derive(Debug, Serialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct EstimateRatioRange {
    pub min: f64,
    pub max: f64,
}

/// Where that set's endpoint rounding agrees. `upper` is `null` when unbounded —
/// never a JSON infinity.
#[derive(Debug, Serialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct EstimateQuantization {
    pub lower: f64,
    #[ts(type = "number | null")]
    pub upper: Option<f64>,
}

/// What was weighed, in codes and counts. The frontend writes the prose; this
/// never does.
#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LimitEstimateExplanation {
    pub reason_codes: Vec<ReasonCode>,
    /// Aggregated to counts so a page load stays bounded however long a Series
    /// has been running.
    pub rejections: Vec<EstimateRejection>,
    #[ts(type = "number")]
    pub qualifying_epochs: usize,
    /// Always three — the contract pins the figure, not merely its type.
    #[ts(type = "3")]
    pub required_epochs: usize,
    #[ts(type = "number")]
    pub recent_cutoff_at: i64,
    #[ts(type = "number | null")]
    pub newest_completed_epoch_at: Option<i64>,
    /// At most five.
    pub candidates: Vec<EstimateEpochSummary>,
    pub ratio_range: Option<EstimateRatioRange>,
    pub quantization_intersection: Option<EstimateQuantization>,
}

/// The state-discriminated half of the evaluation (spec: "Public shape").
/// `tokensPerPct` exists on the Ready variant and nowhere else, so the
/// generated TypeScript narrows on `state` and "only Ready serializes a finite
/// positive tokensPerPct" holds by construction rather than by discipline.
#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
#[ts(export, export_to = "../../src/bindings/")]
pub enum LimitEstimateOutcome {
    #[serde(rename_all = "camelCase")]
    Ready { tokens_per_pct: f64 },
    Gathering,
    Unstable,
    Stale,
    Blocked,
}

/// The tagged evaluation every Limit row carries.
///
/// Deliberately absent: pre-rounded used/left figures and any 100% equivalent —
/// the frontend derives those from the percentage it is already showing.
#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LimitEstimateEvaluation {
    /// Flattened, so the wire shape is the spec's union: `state` at the top
    /// level, with `tokensPerPct` beside it exactly when `state == "ready"`.
    #[serde(flatten)]
    #[ts(flatten)]
    pub outcome: LimitEstimateOutcome,
    #[ts(type = "number")]
    pub evaluated_at: i64,
    #[ts(type = "number | null")]
    pub next_evaluation_at: Option<i64>,
    #[ts(type = "\"limit-token-estimate-v1\"")]
    pub policy_version: String,
    pub explanation: LimitEstimateExplanation,
}

#[derive(Debug, Serialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct SourceLimits {
    pub source: String,
    /// `rateLimitTier` (Claude) / `plan_type` (Codex) as of the newest Reading.
    pub plan: Option<String>,
    /// Current Codex Usage Reset count; filled from the live Artifact by the
    /// command layer because it is state, not Reading history.
    #[ts(type = "number | null")]
    pub usage_resets_available: Option<u64>,
    pub windows: Vec<LimitWindow>,
}

/// A window's `resets_at` is not a stable epoch identity: within what is plainly
/// one window it jitters by a median of 1 second and occasionally by up to ±117s
/// (#104), because the server appears to recompute it per response. Two Readings
/// whose stamps differ by a couple of minutes are the same epoch, so the newest
/// epoch is a *band* below `MAX(resets_at)` rather than one exact value —
/// otherwise the "current" figure would be whichever jittered row happened to
/// name the largest stamp. Ten minutes is far wider than the observed wobble and
/// far narrower than the shortest window Codex reports (300 minutes).
pub(crate) const EPOCH_JITTER_SECS: i64 = 600;

/// How many Readings the Stale reconstruction pages in at a time when it walks
/// history the bounded read never covered. Large enough that real histories
/// take a handful of pages; small enough that no page holds a whole Ledger's
/// Readings and Usage at once.
const STALE_PAGE_READINGS: usize = 2_000;

/// The plan pill's text, as of a Source's newest observation. Exported for the
/// same reason as the statement below it: a profile must EXPLAIN what runs.
pub const PLAN_LABEL_SQL: &str =
    "SELECT plan FROM limit_readings WHERE source = ?1 AND plan IS NOT NULL \
     ORDER BY observed_at DESC LIMIT 1";

/// Which window each card draws, and from which Reading — exported so a profile
/// can `EXPLAIN` and time the statement the page actually issues rather than a
/// copy of it. It has no time bound by design: a card shows the newest epoch, and
/// which epoch is newest is a fact about the whole table.
pub const DISPLAYED_WINDOWS_SQL: &str =
    "SELECT r.source, r.window_key, MAX(r.window_minutes), MAX(r.used_pct), \
            MAX(r.resets_at), MAX(r.observed_at) \
     FROM limit_readings r \
     JOIN (SELECT source, window_key, MAX(resets_at) AS newest FROM limit_readings \
           GROUP BY source, window_key) e \
       ON e.source = r.source AND e.window_key = r.window_key \
     WHERE r.resets_at >= e.newest - ?1 \
     GROUP BY r.source, r.window_key \
     ORDER BY r.source, MAX(r.window_minutes), r.window_key";

/// The current state of every Limit the Ledger holds Readings for: per
/// (source, window_key) the newest epoch, and within it the highest `used_pct`
/// — "the newest valid Reading" (CONTEXT.md). `used_pct` is effectively
/// monotonic within an epoch (#104), so the highest is the latest.
///
/// This ignores the Overview's date window and Source selection entirely: the
/// Limits page is *now*, not a range.
///
/// `limit_exports` is the Companion Export Artifact directory: a live Source's
/// current-state facts (Usage Resets) ride the card from there. An empty path
/// means not configured — the same spelling scan::merge_limit_exports guards —
/// and skips the read entirely.
pub fn limits(
    conn: &Connection,
    evaluated_at: i64,
    limit_exports: &std::path::Path,
) -> Result<Vec<SourceLimits>, LimitsError> {
    // One snapshot for the whole page. Four statements answer it — the rows, the
    // Readings, their Usage, the plan — and a scan committing between them would
    // otherwise let a row be drawn from one view of the database and its estimate
    // from another.
    let read = conn.unchecked_transaction()?;
    let mut stmt = conn.prepare(DISPLAYED_WINDOWS_SQL)?;
    let rows = stmt.query_map([EPOCH_JITTER_SECS], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, f64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;
    let displayed: Vec<(String, String, Option<i64>, f64, i64, i64)> =
        rows.collect::<rusqlite::Result<_>>()?;

    // One horizon for the whole read, from the longest window on the page: one
    // recency horizon for the candidates a Ready answer needs, and one more
    // behind it so the ordinary Stale — a core that aged out recently — is
    // found without paging. Older history is not unreachable: a Gathering
    // answer pages backwards through it below.
    let longest = displayed.iter().filter_map(|w| w.2).max();
    let since = evaluated_at - 2 * recency_horizon(longest);
    let readings = limits_evidence::stored_readings(conn, since)?;
    let usage = limits_evidence::matching_usage(conn, &readings)?;
    // An invariant failure is a technical error, not a readiness state: it
    // rejects the whole command rather than being shown as Blocked.
    let broken = |invariant: limits_evidence::NonFinitePercentage| {
        LimitsError::Invariant(format!(
            "{} reported a percentage that is not a number, observed at {}",
            invariant.source, invariant.observed_at
        ))
    };
    let mut evidence = limits_evidence::derive(&readings, &usage).map_err(broken)?;

    let mut evaluated = Vec::with_capacity(displayed.len());
    for (source, window_key, window_minutes, used_pct, resets_at, observed_at) in displayed {
        // The Reading the ESTIMATE evaluates from: the newest one that proves
        // its Series, independent of the display Reading (which stays the
        // newest overall, chosen by the SQL above). Codex interleaves
        // account-less rollout Readings between the Companion's proven ones,
        // so newest-overall flapped the estimate to Blocked
        // (missing-account-identity) on every request while the Companion was
        // signed in (#183). With no identity-bearing Reading at all, the
        // newest overall stands, so Blocked still names the missing fact.
        let newest = |proving: bool| {
            readings
                .iter()
                .filter(|r| r.source == source && r.window_key == window_key)
                .filter(|r| !proving || SeriesKey::of(r).is_ok())
                .max_by_key(|r| (r.observed_at, r.used_pct.to_bits()))
        };
        let current = newest(true).or_else(|| newest(false));
        let evaluation = limits_readiness::evaluate(current, &evidence.partitions, evaluated_at);
        evaluated
            .push((source, window_key, window_minutes, used_pct, resets_at, observed_at, current, evaluation));
    }

    // Gathering is the one answer that may only mean "not read far enough":
    // Stale is reconstructed by replaying the policy over completed history,
    // newest-first, stopping at the first Ready proof or when history is
    // exhausted (spec: "Readiness state machine"), and the bounded read above
    // reaches two horizons back at most. So a Gathering window pages older
    // history in: each page's Readings and Usage are read, derived, and
    // dropped, and only the Partition summaries accumulate — which is the
    // memory posture the specification asks for. Every other state is final on
    // the bounded read, and a Ready page never enters this loop.
    // ponytail: each page re-runs `evaluate`, whose Stale replay walks every
    // accumulated clock again — quadratic in a Series' completed epochs. Bound
    // the re-check to each batch's own clocks if a Series ever holds enough
    // identity-bearing history for it to show.
    let mut partitions = std::mem::take(&mut evidence.partitions);
    let mut cursor = (since, i64::MIN);
    while evaluated.iter().any(|w| w.7.state == ReadinessState::Gathering) {
        let (older, next) =
            limits_evidence::stored_readings_page(conn, cursor, STALE_PAGE_READINGS)?;
        let Some(next) = next else { break };
        cursor = next;
        let older_usage = limits_evidence::matching_usage(conn, &older)?;
        let paged = limits_evidence::derive(&older, &older_usage).map_err(broken)?;
        limits_evidence::absorb(&mut partitions, paged.partitions);
        for window in &mut evaluated {
            if window.7.state == ReadinessState::Gathering {
                window.7 = limits_readiness::evaluate(window.6, &partitions, evaluated_at);
            }
        }
    }

    let mut cards: Vec<SourceLimits> = Vec::new();
    for (source, window_key, window_minutes, used_pct, resets_at, observed_at, current, evaluation) in
        evaluated
    {
        // Everything this Limit's evidence refused, beside everything its
        // estimator did: the interval and Reading refusals are most of the
        // twenty-two reasons there are, and a page that reported only the
        // estimator's would explain almost nothing.
        let refusals = evidence.refusals(&source, &window_key);
        let window = LimitWindow {
            window_key,
            window_minutes,
            used_pct,
            resets_at,
            observed_at,
            estimate: on_the_wire(evaluation, current, refusals)?,
        };
        match cards.last_mut() {
            Some(card) if card.source == source => card.windows.push(window),
            _ => cards.push(SourceLimits {
                source,
                plan: None,
                usage_resets_available: None,
                windows: vec![window],
            }),
        }
    }

    // The plan label as of the newest observation of that Source — one pill per
    // card, so one answer per Source rather than a column on every window.
    //
    // Taken from the Readings already in hand wherever they carry it. They are the
    // newest ones there are, so the newest of them naming a plan IS the newest
    // naming a plan; the statement only has to run for a Source whose whole recent
    // history is silent about its plan, which is the case it was written for.
    // Asking SQLite every time cost 23.6 ms of a 71 ms page at 201,300 Readings:
    // it seeks the Source and then sorts every row of it to keep one, and that
    // sort grows with the table for as long as the app runs.
    let mut plan_stmt = conn.prepare(PLAN_LABEL_SQL)?;
    for card in &mut cards {
        card.plan = readings
            .iter()
            .filter(|r| r.source == card.source && r.plan.is_some())
            .max_by_key(|r| r.observed_at)
            .and_then(|r| r.plan.clone());
        if card.plan.is_none() {
            card.plan = plan_stmt
                .query_row([&card.source], |r| r.get(0))
                .optional()?;
        }
    }
    drop(plan_stmt);
    drop(stmt);
    read.finish()?;

    // Usage Resets ride the card from a live Source's Companion Export
    // Artifact (glossary: Usage Reset): current state, not a Limit Reading,
    // so it lives in the Artifact rather than the Ledger's tables, and is
    // read here — after the snapshot commits, since a file was never part of
    // the database's view — so the whole card is still assembled in one
    // function. Driven by the cards over the catalog's `live` capability,
    // mirroring scan::merge_limit_exports: no Source is named, the next live
    // Source that reports resets is a catalog entry, and its same guard
    // applies — an unconfigured dir ("") must not send relative lookups
    // through the process CWD. An absent export or count leaves the field
    // unknown (None), never zero.
    if !limit_exports.as_os_str().is_empty() {
        for card in &mut cards {
            let live = crate::source_catalog::source(&card.source)
                .is_some_and(|s| s.capabilities.limits.as_deref() == Some("live"));
            if !live {
                continue;
            }
            if let Some(export) = crate::limits_artifact::read(limit_exports, &card.source) {
                card.usage_resets_available = export.usage_resets_available;
            }
        }
    }

    Ok(cards)
}

/// An evaluation, reduced to what the page is allowed to see.
fn on_the_wire(
    evaluation: Evaluation,
    current: Option<&LimitReading>,
    evidence_refusals: BTreeMap<ReasonCode, usize>,
) -> Result<LimitEstimateEvaluation, LimitsError> {
    let explanation = evaluation.explanation;
    // A candidate exists only where the current Reading proved its Series, so
    // there is always a key to make one with.
    let series = current.and_then(|r| SeriesKey::of(r).ok());
    let candidates = series
        .as_ref()
        .map(|series| {
            explanation
                .candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let in_core = explanation.core.contains(&index);
                    // Why the core left this one out, in the estimator's own
                    // judgment: the core with this candidate added back either
                    // shares no endpoint rounding or exceeds the spread. A core
                    // member — or any candidate when no core formed — has
                    // nothing to answer for, and says nothing.
                    let reason_codes = if in_core || explanation.core.is_empty() {
                        Vec::new()
                    } else {
                        let mut with: Vec<&Candidate> = explanation
                            .core
                            .iter()
                            .map(|&i| &explanation.candidates[i])
                            .collect();
                        with.push(candidate);
                        coheres(&with).into_iter().collect()
                    };
                    EstimateEpochSummary {
                        epoch_key: epoch_key(series, candidate.epoch_ended_at),
                        ended_at: candidate.epoch_ended_at,
                        movement_points: candidate.movement,
                        positive_movements: candidate.positive_movements,
                        in_core,
                        reason_codes,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // One tally per reason, whichever stage refused it.
    let mut rejections = evidence_refusals;
    for (reason, count) in explanation.rejections {
        *rejections.entry(reason).or_insert(0) += count;
    }

    // Only Ready carries a number, held by the variant's own shape — and it
    // must be a token count: a Ready evaluation without a finite positive
    // ratio is not a withheld state but a fault, and the specification calls
    // it an invariant failure, so it rejects the command rather than shipping
    // a Ready row with nothing in it.
    let outcome = match evaluation.state {
        ReadinessState::Ready => match evaluation.tokens_per_pct {
            Some(ratio) if ratio.is_finite() && ratio > 0.0 => {
                LimitEstimateOutcome::Ready { tokens_per_pct: ratio }
            }
            broken => {
                return Err(LimitsError::Invariant(format!(
                    "a Ready estimate resolved to {broken:?}, which is not a token count",
                )))
            }
        },
        ReadinessState::Gathering => LimitEstimateOutcome::Gathering,
        ReadinessState::Unstable => LimitEstimateOutcome::Unstable,
        ReadinessState::Stale => LimitEstimateOutcome::Stale,
        ReadinessState::Blocked => LimitEstimateOutcome::Blocked,
    };

    Ok(LimitEstimateEvaluation {
        outcome,
        evaluated_at: evaluation.evaluated_at,
        next_evaluation_at: evaluation.next_evaluation_at,
        policy_version: evaluation.policy_version.to_string(),
        explanation: LimitEstimateExplanation {
            reason_codes: explanation.reason_codes,
            rejections: rejections
                .into_iter()
                .map(|(reason_code, count)| EstimateRejection { reason_code, count })
                .collect(),
            qualifying_epochs: explanation.qualifying_epochs,
            required_epochs: explanation.required_epochs,
            recent_cutoff_at: explanation.recent_cutoff_at,
            newest_completed_epoch_at: explanation.newest_completed_epoch_at,
            candidates,
            ratio_range: explanation
                .ratio_range
                .map(|(min, max)| EstimateRatioRange { min, max }),
            quantization_intersection: explanation.quantization_intersection.map(|q| {
                EstimateQuantization { lower: q.lower, upper: q.upper }
            }),
        },
    })
}

/// A diagnostic identity for one epoch of one Series: enough to tell two apart
/// and to recognise the same one twice, and nothing that can be read back into
/// an account. The Series carries an opaque account identity, so it is digested
/// rather than sent.
fn epoch_key(series: &SeriesKey, epoch: i64) -> String {
    let mut digest = Sha256::new();
    for part in [
        series.source.as_str(),
        series.account_id.as_str(),
        series.plan.as_str(),
        series.metering_regime.as_str(),
        series.limit_id.as_str(),
        series.model_scope.as_str(),
    ] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    digest.update(epoch.to_be_bytes());
    format!("{:x}", digest.finalize())[..16].to_string()
}

/// What the Limits query can fail with. A storage fault and a broken invariant
/// are both technical errors — neither is a readiness state, and neither may be
/// shown as Blocked.
#[derive(Debug)]
pub enum LimitsError {
    Sqlite(rusqlite::Error),
    Invariant(String),
}

impl From<rusqlite::Error> for LimitsError {
    fn from(error: rusqlite::Error) -> Self {
        LimitsError::Sqlite(error)
    }
}

impl std::fmt::Display for LimitsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitsError::Sqlite(error) => write!(f, "{error}"),
            LimitsError::Invariant(detail) => write!(f, "{detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::pricing::{self, OverrideRates};
    use crate::types::{LimitReading, ReadingProvenance, UsageEvent};
    use tempfile::tempdir;

    // 2026-07-01T12:00:00Z and 2026-07-02T12:00:00Z (event times)
    const DAY1_TS: i64 = 1_782_907_200;
    const DAY2_TS: i64 = 1_782_993_600;
    // 2026-07-01T00:00:00Z and 2026-07-02T00:00:00Z (local-midnight bounds under TZ=UTC)
    const DAY1_START: i64 = 1_782_864_000;
    const DAY2_START: i64 = 1_782_950_400;

    /// The instant the Limits tests evaluate at — later than every fixture
    /// Reading, so a card is drawn from history rather than from the future.
    const EVALUATED_AT: i64 = 1_900_000_000;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[allow(clippy::too_many_arguments)]
    fn ev(
        key: &str, source: &str, ts: i64, model: &str, project: Option<&str>,
        calls: i64, input: i64, output: i64, cr: i64, w5: i64, w1: i64,
    ) -> UsageEvent {
        UsageEvent {
            dedup_key: key.to_string(),
            source: source.to_string(),
            timestamp: ts,
            model: Some(model.to_string()),
            project: project.map(|p| p.to_string()),
            api_calls: calls,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cr,
            cache_write_5m_tokens: w5,
            cache_write_1h_tokens: w1,
            source_file: "fixture.jsonl".to_string(),
            session_id: None,
            reasoning_tokens: None,
            ctx: Default::default(),
        }
    }

    // Seed: two priced gpt-5.4 events (day1 + day2, project alpha, source codex)
    // and one unpriced hermes-local event (day1, no project, source hermes,
    // api_call_count = 3). Prices for gpt-5.4 inserted directly.
    fn seed() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let events = vec![
            ev("a", "codex", DAY1_TS, "gpt-5.4", Some("/Users/dev/projects/alpha"), 1, 1000, 500, 200, 100, 50),
            ev("b", "codex", DAY2_TS, "gpt-5.4", Some("/Users/dev/projects/alpha"), 1, 2000, 1000, 0, 0, 0),
            ev("c", "hermes", DAY1_TS, "hermes-local", None, 3, 300, 100, 0, 0, 0),
        ];
        db::insert_events(&mut conn, &events).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('gpt-5.4', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
            [],
        ).unwrap();
        (dir, conn)
    }

    #[test]
    fn summary_totals_cost_and_unpriced() {
        let (_dir, conn) = seed();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.input_tokens, 3300);
        assert_eq!(s.output_tokens, 1600);
        assert_eq!(s.cache_read_tokens, 200);
        assert_eq!(s.cache_write_tokens, 150);
        assert_eq!(s.total_tokens, 5250);
        assert_eq!(s.requests, 5);
        // gpt-5.4 agg: in3000 out1500 cr200 w5=100 w1=50
        // = 0.006 + 0.015 + 0.0001 + 0.00025 + 0.0002
        approx(s.cost.unwrap(), 0.02155);
        assert!(s.has_unpriced);
        assert_eq!(s.unpriced_models, vec!["hermes-local".to_string()]);
        assert_eq!(s.unattributed_tokens, 0, "current Sources attribute every Usage Record to a Model");
        approx(s.cache_hit_rate, 200.0 / 3650.0); // cr / (input + cr + cache_write)
    }

    #[test]
    fn auto_review_cost_uses_each_days_frozen_price_on_every_surface() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_events(
            &mut conn,
            &[
                ev("review-1", "codex", DAY1_TS, "codex-auto-review", Some("/p"), 1, 100, 0, 0, 0, 0),
                ev("review-2", "codex", DAY2_TS, "codex-auto-review", Some("/p"), 1, 100, 0, 0, 0, 0),
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, catalog) \
             VALUES ('codex-auto-review', 0.000009, 0, 'litellm')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO model_price_history (model, day, priced, input_per_tok) \
             VALUES ('codex-auto-review', '2026-07-01', 1, 0.000001); \
             INSERT INTO model_price_history (model, day, priced, input_per_tok) \
             VALUES ('codex-auto-review', '2026-07-02', 1, 0.000002);",
        )
        .unwrap();

        approx(summary(&conn, &Filters::default()).unwrap().cost.unwrap(), 0.0003);

        let series_rows = series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series_rows.len(), 2);
        approx(series_rows[0].cost, 0.0001);
        approx(series_rows[1].cost, 0.0002);

        let model_rows = breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(model_rows.len(), 1);
        approx(model_rows[0].cost.unwrap(), 0.0003);
    }

    // The two zeroes a window can hold, kept apart: nothing recorded costs
    // zero (every surface shows $0.00), while usage nothing can price has no
    // Cost at all (every surface shows "unpriced", never $0).
    #[test]
    fn empty_window_costs_zero_while_unpriceable_usage_has_no_cost() {
        let (_dir, conn) = seed();
        let empty = Filters {
            project: Some("/p/never-used".to_string()),
            ..Filters::default()
        };
        let s = summary(&conn, &empty).unwrap();
        assert_eq!(s.total_tokens, 0);
        assert_eq!(s.cost, Some(0.0));

        // seed's hermes-local has no rate: tokens present, Cost unavailable.
        let unpriceable = Filters {
            models: vec!["hermes-local".to_string()],
            ..Filters::default()
        };
        let s = summary(&conn, &unpriceable).unwrap();
        assert!(s.total_tokens > 0);
        assert_eq!(s.cost, None);
    }

    #[test]
    fn summary_counts_all_unattributed_usage_with_unavailable_cost() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut usage = ev(
            "pi:tool-result:1", "pi", DAY1_TS, "unused", Some("/p/pi"),
            2, 100, 50, 20, 10, 5,
        );
        usage.model = None;
        db::insert_events(&mut conn, &[usage]).unwrap();

        let filters = Filters {
            tools: vec!["pi".to_string()],
            project: Some("/p/pi".to_string()),
            start_ts: Some(DAY1_START),
            end_ts: Some(DAY2_START),
            ..Filters::default()
        };
        let s = summary(&conn, &filters).unwrap();
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 50);
        assert_eq!(s.cache_read_tokens, 20);
        assert_eq!(s.cache_write_tokens, 15);
        assert_eq!(s.total_tokens, 185);
        assert_eq!(s.requests, 2);
        assert_eq!(s.cost, None);
        assert!(!s.has_unpriced, "Unattributed Usage is not an Unpriced Model");
        assert!(s.unpriced_models.is_empty());
        assert_eq!(s.unattributed_tokens, 185);
    }

    #[test]
    fn summary_keeps_unpriced_and_unattributed_reasons_separate_and_model_filters_omit_null() {
        let (_dir, mut conn) = seed();
        let mut usage = ev(
            "pi:tool-result:1", "pi", DAY1_TS, "unused", None,
            4, 75, 25, 0, 0, 0,
        );
        usage.model = None;
        db::insert_events(&mut conn, &[usage]).unwrap();

        let s = summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.total_tokens, 5350);
        assert_eq!(s.requests, 9);
        approx(s.cost.unwrap(), 0.02155);
        assert!(s.has_unpriced);
        assert_eq!(s.unpriced_models, vec!["hermes-local".to_string()]);
        assert_eq!(s.unattributed_tokens, 100);

        let model_only = summary(&conn, &Filters {
            models: vec!["gpt-5.4".to_string()],
            ..Filters::default()
        }).unwrap();
        assert_eq!(model_only.total_tokens, 4850);
        assert_eq!(model_only.requests, 2);
        assert_eq!(model_only.unattributed_tokens, 0);
        assert!(!model_only.has_unpriced);
    }

    #[test]
    fn summary_tool_filter_excludes_unpriced() {
        let (_dir, conn) = seed();
        let f = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        let s = summary(&conn, &f).unwrap();
        assert_eq!(s.total_tokens, 4850);
        assert_eq!(s.requests, 2);
        approx(s.cost.unwrap(), 0.02155);
        assert!(!s.has_unpriced);
        assert!(s.unpriced_models.is_empty());
        assert_eq!(s.unattributed_tokens, 0);
    }

    #[test]
    fn summary_end_ts_is_exclusive() {
        let (_dir, conn) = seed();
        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let s = summary(&conn, &f).unwrap();
        assert_eq!(s.total_tokens, 2250); // only day-1 events A + C; day-2 B excluded
        assert_eq!(s.requests, 4);
        approx(s.cost.unwrap(), 0.00755); // event A only
        assert!(s.has_unpriced);
    }

    #[test]
    fn override_prices_previously_unpriced_model() {
        let (_dir, conn) = seed();
        pricing::set_override(&conn, "hermes-local", OverrideRates {
            input: Some(0.000001), output: None, cache_read: None, cache_write: None,
        }).unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert!(!s.has_unpriced);
        assert!(s.unpriced_models.is_empty());
        approx(s.cost.unwrap(), 0.02185); // 0.02155 + 300 * 0.000001
    }

    #[test]
    fn breakdown_preserves_unattributed_identity_and_counts_it_in_source_and_project() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('gpt-5.4', 0.000002, 0.000010, 0, 0, 0)",
            [],
        ).unwrap();
        let mut priced = ev("a", "pi", DAY1_TS, "gpt-5.4", Some("/p/pi"), 1, 100, 50, 0, 0, 0);
        priced.session_id = Some("s1".to_string());
        let mut unpriced = ev("b", "pi", DAY1_TS, "local", Some("/p/pi"), 1, 30, 20, 0, 0, 0);
        unpriced.session_id = Some("s2".to_string());
        let mut unattributed = ev("c", "pi", DAY1_TS, "unused", Some("/p/pi"), 2, 40, 10, 0, 0, 0);
        unattributed.model = None;
        unattributed.session_id = Some("s3".to_string());
        db::insert_events(&mut conn, &[priced, unpriced, unattributed]).unwrap();

        let model_rows = breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(model_rows.len(), 3);
        let null_model = model_rows.iter().find(|r| r.key.is_none()).unwrap();
        assert_eq!(null_model.source.as_deref(), Some("pi"));
        assert_eq!(null_model.total_tokens, 50);
        assert_eq!(null_model.requests, 2);
        assert_eq!(null_model.cost, None);
        assert!(!null_model.has_unpriced);
        assert_eq!(null_model.unattributed_tokens, 50);
        assert_eq!(null_model.convs, 1);
        let local = model_rows.iter().find(|r| r.key.as_deref() == Some("local")).unwrap();
        assert!(local.has_unpriced);
        assert_eq!(local.unattributed_tokens, 0);

        for by in ["tool", "project"] {
            let rows = breakdown(&conn, by, &Filters::default()).unwrap();
            assert_eq!(rows.len(), 1, "one {by} row");
            let row = &rows[0];
            assert_eq!(row.total_tokens, 250);
            assert_eq!(row.requests, 4);
            approx(row.cost.unwrap(), 0.0007);
            assert!(row.has_unpriced);
            assert_eq!(row.unattributed_tokens, 50);
            assert_eq!(row.convs, 3);
        }
    }

    #[test]
    fn breakdown_by_model_sorted_desc_with_none_cost() {
        let (_dir, conn) = seed();
        let rows = breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key.as_deref(), Some("gpt-5.4"));
        assert_eq!(rows[0].unattributed_tokens, 0);
        assert_eq!(rows[0].total_tokens, 4850);
        assert_eq!(rows[0].requests, 2);
        approx(rows[0].cost.unwrap(), 0.02155);
        assert_eq!(rows[1].key.as_deref(), Some("hermes-local"));
        assert_eq!(rows[1].unattributed_tokens, 0);
        assert_eq!(rows[1].total_tokens, 400);
        assert_eq!(rows[1].requests, 3);
        assert!(rows[1].cost.is_none());
        assert_eq!(rows[0].source, Some("codex".to_string()));
        assert_eq!(rows[1].source, Some("hermes".to_string()));
    }

    #[test]
    fn summary_flags_cache_estimated_models() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // Absent catalog cache rates are stored as 0.0 (see pricing::write_price_row).
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('full-priced', 0.000001, 0.000002, 0.0000001, 0.000001, 0.000001)",
            [],
        ).unwrap();
        let events = vec![
            ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 40, 10, 0),
            ev("b", "codex", DAY1_TS, "full-priced", None, 1, 100, 50, 40, 10, 0),
        ];
        db::insert_events(&mut conn, &events).unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.cache_estimated_models, vec!["half-priced".to_string()]);
        assert!(!s.has_unpriced);
        assert!(s.cost.is_some());
        let pts = series(&conn, &Filters::default(), "day").unwrap();
        assert!(pts.iter().any(|p| p.cache_estimated), "series rides the same flag");
    }

    #[test]
    fn cache_estimated_requires_cache_tokens() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        // No cache tokens at all -> nothing is missing from the estimate.
        db::insert_events(&mut conn, &[ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0)]).unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert!(s.cache_estimated_models.is_empty());
        let pts = series(&conn, &Filters::default(), "day").unwrap();
        assert!(pts.iter().all(|p| !p.cache_estimated));
    }

    #[test]
    fn breakdown_model_rows_carry_source_convs_reasoning_and_flag() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        let mut e1 = ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 40, 0, 0);
        e1.session_id = Some("sa".to_string());
        e1.reasoning_tokens = Some(5);
        let mut e2 = ev("b", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0);
        e2.session_id = Some("sa".to_string());
        e2.reasoning_tokens = Some(3);
        // Same model name from a different source -> its own row.
        let mut e3 = ev("c", "hermes", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0);
        e3.session_id = Some("hs".to_string());
        db::insert_events(&mut conn, &[e1, e2, e3]).unwrap();

        let rows = breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(rows.len(), 2, "model rows split by source");
        let codex = rows.iter().find(|r| r.source == Some("codex".to_string())).unwrap();
        assert_eq!(codex.key.as_deref(), Some("half-priced"));
        assert_eq!(codex.convs, 1, "one distinct session");
        assert_eq!(codex.reasoning_tokens, Some(8));
        assert!(codex.cache_estimated, "cache tokens present but cache rate is 0");
        let hermes = rows.iter().find(|r| r.source == Some("hermes".to_string())).unwrap();
        assert_eq!(hermes.convs, 1);
        assert_eq!(hermes.reasoning_tokens, None);
        assert!(!hermes.cache_estimated, "no cache tokens used");
    }

    #[test]
    fn breakdown_project_carries_convs_and_null_source() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut e1 = ev("a", "codex", DAY1_TS, "gpt-5.4", Some("/p/alpha"), 1, 100, 50, 0, 0, 0);
        e1.session_id = Some("sa".to_string());
        let mut e2 = ev("b", "codex", DAY1_TS, "gpt-5.4-mini", Some("/p/alpha"), 1, 100, 50, 0, 0, 0);
        e2.session_id = Some("sa".to_string());
        db::insert_events(&mut conn, &[e1, e2]).unwrap();
        let rows = breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, None, "source only set for model mode");
        assert_eq!(rows[0].convs, 1, "distinct across models within the project");
    }

    #[test]
    fn breakdown_by_project_maps_null_to_unknown() {
        let (_dir, conn) = seed();
        let rows = breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(rows[0].key.as_deref(), Some("/Users/dev/projects/alpha"));
        assert_eq!(rows[0].total_tokens, 4850);
        assert_eq!(rows[1].key.as_deref(), Some("unknown"));
        assert_eq!(rows[1].total_tokens, 400);
    }

    #[test]
    fn window_matches_the_four_priced_reads() {
        let (_dir, conn) = seed();
        let f = Filters::default();
        let w = window(&conn, &f).unwrap();
        let s = summary(&conn, &f).unwrap();
        assert_eq!(w.summary.total_tokens, s.total_tokens);
        assert_eq!(w.summary.cost, s.cost);
        assert_eq!(w.summary.has_unpriced, s.has_unpriced);
        assert_eq!(w.summary.unattributed_tokens, s.unattributed_tokens);
        assert_eq!(w.summary.convs, s.convs);
        let keys = |rows: &[BreakdownRow]| {
            rows.iter().map(|r| (r.key.clone(), r.source.clone(), r.total_tokens, r.cost)).collect::<Vec<_>>()
        };
        assert_eq!(keys(&w.models), keys(&breakdown(&conn, "model", &f).unwrap()));
        assert_eq!(keys(&w.projects), keys(&breakdown(&conn, "project", &f).unwrap()));
        assert_eq!(keys(&w.sources), keys(&breakdown(&conn, "tool", &f).unwrap()));
    }

    // Events with v2 fields for series tests.
    fn ev_s(
        key: &str, source: &str, ts: i64, model: &str,
        session: Option<&str>, reasoning: Option<i64>,
    ) -> UsageEvent {
        let mut e = ev(key, source, ts, model, None, 1, 100, 50, 0, 0, 0);
        e.session_id = session.map(|s| s.to_string());
        e.reasoning_tokens = reasoning;
        e
    }

    #[test]
    fn series_keeps_unattributed_usage_outside_the_model_map() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let attributed = ev_s(
            "pi:assistant:1", "pi", DAY1_TS, "gpt-5.4", Some("s1"), Some(5),
        );
        let mut unattributed = ev_s(
            "pi:tool-result:1", "pi", DAY1_TS, "unused", Some("s2"), None,
        );
        unattributed.model = None;
        unattributed.api_calls = 2;
        db::insert_events(&mut conn, &[attributed, unattributed]).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('gpt-5.4', 0.000002, 0.000010, 0, 0, 0)",
            [],
        ).unwrap();

        let pts = series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(pts.len(), 1);
        let point = &pts[0];
        assert_eq!(point.total_tokens, 300);
        assert_eq!(point.by_model.len(), 1);
        assert_eq!(point.by_model.get("gpt-5.4"), Some(&150));
        assert_eq!(point.unattributed_tokens, 150);
        assert!(!point.has_unpriced);
        approx(point.cost, 0.0007);
        assert_eq!(point.requests, 3);
        assert_eq!(point.convs, 2);
        assert_eq!(point.reasoning_tokens, Some(5));
    }

    // The reason Summary carries convs at all: the per-day counts in series()
    // cannot be summed, because a Session that spans days is counted once per
    // day it touches. Summary counts distinct over the whole window instead.
    #[test]
    fn summary_counts_a_session_spanning_days_once() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let events = vec![
            ev_s("a", "codex", DAY1_TS, "gpt-5.4", Some("sa"), None),
            ev_s("b", "codex", DAY2_TS, "gpt-5.4", Some("sa"), None), // same Session, next day
            ev_s("c", "codex", DAY2_TS, "gpt-5.4", Some("sb"), None),
            ev_s("d", "claude", DAY1_TS, "claude-opus-4-6", Some("sa"), None), // same id, other Source
            ev_s("e", "codex", DAY2_TS, "gpt-5.4", None, None), // no Session identity
        ];
        db::insert_events(&mut conn, &events).unwrap();

        let s = summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.convs, 3, "codex:sa + codex:sb + claude:sa");

        let per_day: i64 = series(&conn, &Filters::default(), "day")
            .unwrap()
            .iter()
            .map(|p| p.convs)
            .sum();
        assert_eq!(per_day, 4, "summing per-day counts double-counts the spanning Session");
    }

    #[test]
    fn series_groups_by_day_and_source() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let events = vec![
            ev_s("c1", "codex", DAY1_TS, "gpt-5.4", Some("sa"), Some(5)),
            ev_s("c2", "codex", DAY1_TS, "gpt-5.4", Some("sa"), Some(3)),
            ev_s("c3", "codex", DAY1_TS, "gpt-5.4-mini", Some("sb"), None),
            ev_s("h1", "hermes", DAY1_TS, "hermes-local", Some("hs"), Some(0)),
            ev_s("c4", "codex", DAY2_TS, "gpt-5.4", None, None),
        ];
        db::insert_events(&mut conn, &events).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('gpt-5.4', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
            [],
        ).unwrap();

        let pts = series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(pts.len(), 3); // (day1,codex), (day1,hermes), (day2,codex)

        let d1c = pts.iter().find(|p| p.bucket == "2026-07-01" && p.source == "codex").unwrap();
        assert_eq!(d1c.total_tokens, 450); // 3 events × (100 input + 50 output)
        assert_eq!(d1c.by_model.get("gpt-5.4"), Some(&300));
        assert_eq!(d1c.by_model.get("gpt-5.4-mini"), Some(&150));
        assert_eq!(d1c.unattributed_tokens, 0);
        assert!(d1c.has_unpriced, "gpt-5.4-mini has no rate");
        assert_eq!(d1c.requests, 3);
        assert_eq!(d1c.convs, 2, "sa + sb, distinct across models within the source");
        assert_eq!(d1c.reasoning_tokens, Some(8), "5 + 3; the NULL event does not zero it");
        // Only the two gpt-5.4 events price: 200×2e-6 + 100×1e-5.
        approx(d1c.cost, 0.0014);

        let d1h = pts.iter().find(|p| p.bucket == "2026-07-01" && p.source == "hermes").unwrap();
        assert_eq!(d1h.reasoning_tokens, Some(0), "reported zero ≠ not reported");
        assert_eq!(d1h.unattributed_tokens, 0);
        assert!(d1h.has_unpriced);
        approx(d1h.cost, 0.0);

        let d2c = pts.iter().find(|p| p.bucket == "2026-07-02").unwrap();
        assert_eq!(d2c.convs, 0, "NULL session ids count zero distinct");
        assert_eq!(d2c.reasoning_tokens, None, "nothing reported that day");
        assert_eq!(d2c.unattributed_tokens, 0);
        assert!(!d2c.has_unpriced);
    }

    #[test]
    fn series_hour_buckets_local_time() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_events(&mut conn, &[ev_s("a", "codex", DAY1_TS, "gpt-5.4", None, None)]).unwrap();
        let pts = series(&conn, &Filters::default(), "hour").unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].bucket, "2026-07-01 12:00");
    }

    #[test]
    fn series_day_sums_match_summary() {
        let (_dir, conn) = seed();
        let pts = series(&conn, &Filters::default(), "day").unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        let total: i64 = pts.iter().map(|p| p.total_tokens).sum();
        assert_eq!(total, s.total_tokens);
        let cost: f64 = pts.iter().map(|p| p.cost).sum();
        approx(cost, s.cost.unwrap());
        let reqs: i64 = pts.iter().map(|p| p.requests).sum();
        assert_eq!(reqs, s.requests);
    }

    #[test]
    fn context_totals_sum_ctx_preserving_null() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut a = ev_s("a", "claude", DAY1_TS, "claude-opus-4-8", Some("s1"), None);
        a.ctx.messages = Some(900);
        a.ctx.system = Some(80);
        a.ctx.reasoning = Some(20);
        a.ctx.toolcalls = Some(300);
        let mut b = ev_s("b", "claude", DAY1_TS, "claude-opus-4-8", Some("s1"), None);
        b.ctx.messages = Some(100);
        b.ctx.system = Some(10);
        b.ctx.reasoning = Some(0);
        // hermes: all-NULL ctx must stay NULL, not become 0
        let h = ev_s("h", "hermes", DAY1_TS, "hermes-local", Some("hs"), None);
        db::insert_events(&mut conn, &[a, b, h]).unwrap();

        let totals = context(&conn, &Filters::default()).unwrap().totals;
        let c = totals.iter().find(|t| t.source == "claude").unwrap();
        assert_eq!(c.messages, Some(1000));
        assert_eq!(c.system, Some(90));
        assert_eq!(c.reasoning, Some(20));
        assert_eq!(c.toolcalls, Some(300));
        assert_eq!(c.agents, None, "no contributing value: NULL, never 0");
        let hm = totals.iter().find(|t| t.source == "hermes").unwrap();
        assert_eq!(hm.messages, None);
    }

    #[test]
    fn ctx_resources_distinct_names_in_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("t.db")).unwrap();
        crate::db::record_resources(&conn, "claude", &[
            ("skill", "verify".to_string(), DAY2_TS),
            ("skill", "graphify".to_string(), DAY1_TS),
            ("skill", "graphify".to_string(), DAY2_TS), // same name, new day: still 1 row
            ("mcp_server", "pencil".to_string(), DAY1_TS),
        ]).unwrap();

        let all = ctx_resources(&conn, &Filters::default()).unwrap();
        let skills: Vec<&str> =
            all.iter().filter(|r| r.kind == "skill").map(|r| r.name.as_str()).collect();
        assert_eq!(skills, ["graphify", "verify"], "deduped across days, name-ordered");
        let mcps: Vec<&str> =
            all.iter().filter(|r| r.kind == "mcp_server").map(|r| r.name.as_str()).collect();
        assert_eq!(mcps, ["pencil"]);

        // Day-1-only window excludes the day-2 'verify'.
        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_resources(&conn, &f).unwrap();
        let d1_skills: Vec<&str> =
            d1.iter().filter(|r| r.kind == "skill").map(|r| r.name.as_str()).collect();
        assert_eq!(d1_skills, ["graphify"]);

        // Tool filter scopes by source.
        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        assert!(ctx_resources(&conn, &f2).unwrap().is_empty());
    }

    #[test]
    fn ctx_buckets_exact_partition_and_first_cw() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // Session sa: first cw event (day1) then a later cw event (day2).
        let mut a = ev("a", "claude", DAY1_TS, "m", None, 1, 100, 50, 0, 900, 0);
        a.session_id = Some("sa".to_string());
        let mut b = ev("b", "claude", DAY2_TS, "m", None, 1, 200, 30, 1500, 250, 0);
        b.session_id = Some("sa".to_string());
        // NULL session id: cache writes count as history, never system.
        let c = ev("c", "claude", DAY1_TS, "m", None, 1, 10, 5, 0, 40, 0);
        // Hermes: aggregated rows — all cw is history, system NULL.
        let mut h = ev("h", "hermes", DAY1_TS, "hermes-local", None, 1, 300, 100, 20, 60, 0);
        h.session_id = Some("hs".to_string());
        h.reasoning_tokens = Some(25);
        // Tie-break: two cw events share session sb's first timestamp — the
        // smaller dedup_key is the session's first cache write (System).
        let mut t1 = ev("t1", "codex", DAY1_TS, "m", None, 1, 0, 0, 0, 70, 0);
        t1.session_id = Some("sb".to_string());
        let mut t2 = ev("t2", "codex", DAY1_TS, "m", None, 1, 0, 0, 0, 500, 0);
        t2.session_id = Some("sb".to_string());
        db::insert_events(&mut conn, &[a, b, c, h, t1, t2]).unwrap();

        let all = ctx_buckets(&conn, &Filters::default()).unwrap();
        let cl = all.iter().find(|x| x.source == "claude").unwrap();
        assert_eq!(cl.system, Some(900), "session sa's FIRST cache write only");
        assert_eq!(cl.history, 1500 + 250 + 40, "cache_read + later cw + NULL-session cw");
        assert_eq!(cl.new_input, 310);
        assert_eq!(cl.reasoning, None, "claude reasoning not reported");
        assert_eq!(cl.response, 85);
        // Exact partition vs total usage.
        let total = 100 + 50 + 900 + 200 + 30 + 1500 + 250 + 10 + 5 + 40;
        assert_eq!(cl.history + cl.new_input + cl.system.unwrap_or(0) + cl.response
            + cl.reasoning.unwrap_or(0), total);

        let cx = all.iter().find(|x| x.source == "codex").unwrap();
        assert_eq!(cx.system, Some(70), "same-timestamp tie breaks on dedup_key");
        assert_eq!(cx.history, 500);

        let hm = all.iter().find(|x| x.source == "hermes").unwrap();
        assert_eq!(hm.system, None, "hermes aggregates: first-vs-rest unknowable");
        assert_eq!(hm.history, 20 + 60, "all hermes cw is history");
        assert_eq!(hm.reasoning, Some(25));
        assert_eq!(hm.response, 75);

        // Range starting day2: session sa's first cw is OUTSIDE the range →
        // in-range cw counts as history, system is NULL (nothing in range).
        let f = Filters { start_ts: Some(DAY2_START), ..Filters::default() };
        let d2 = ctx_buckets(&conn, &f).unwrap();
        let cl2 = d2.iter().find(|x| x.source == "claude").unwrap();
        assert_eq!(cl2.system, None);
        assert_eq!(cl2.history, 1500 + 250);
    }

    #[test]
    fn ctx_tools_sums_by_source_and_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::add_ctx_tool_rows(&mut conn, "claude", "f1", &[
            ("Bash".to_string(), 100, 2, DAY1_TS),
            ("Bash".to_string(), 50, 1, DAY2_TS),
            ("Read".to_string(), 30, 1, DAY1_TS),
        ]).unwrap();
        db::add_ctx_tool_rows(&mut conn, "codex", "f2", &[
            ("shell".to_string(), 70, 1, DAY1_TS),
        ]).unwrap();

        let all = ctx_tools(&conn, &Filters::default()).unwrap();
        let bash = all.iter().find(|r| r.name == "Bash").unwrap();
        assert_eq!((bash.est_tokens, bash.calls), (150, 3));

        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_tools(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.name == "Bash").unwrap().est_tokens, 100);

        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        let cx = ctx_tools(&conn, &f2).unwrap();
        assert_eq!(cx.len(), 1);
        assert_eq!(cx[0].name, "shell");
    }

    #[test]
    fn ctx_skills_sums_by_name_heaviest_first() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::add_ctx_skill_rows(&mut conn, "claude", "f1", &[
            ("grilling".to_string(), 2400, 1, DAY1_TS),
            ("grilling".to_string(), 2400, 1, DAY2_TS),
            ("playground:playground".to_string(), 900, 1, DAY1_TS),
        ]).unwrap();

        // Summed across days and files, heaviest first — the ordering is what
        // lets the panel answer "which skill is costing me".
        let all = ctx_skills(&conn, &Filters::default()).unwrap();
        assert_eq!(all[0].name, "grilling");
        assert_eq!((all[0].est_tokens, all[0].uses), (4800, 2));
        assert_eq!(all[1].name, "playground:playground");

        // Day window narrows it like every other ctx query.
        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_skills(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.name == "grilling").unwrap().est_tokens, 2400);
    }

    #[test]
    fn ctx_exec_sums_by_key_and_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::add_ctx_exec_rows(&mut conn, "claude", "f1", &[
            ("git_local".into(), "git".into(), "git add".into(), 100, 1, DAY1_TS),
            ("git_local".into(), "git".into(), "git add".into(), 50, 1, DAY2_TS),
            ("test".into(), "npm".into(), "npm test".into(), 30, 1, DAY1_TS),
        ]).unwrap();

        let all = ctx_exec(&conn, &Filters::default()).unwrap();
        let ga = all.iter().find(|r| r.cmd == "git add").unwrap();
        assert_eq!((ga.est_tokens, ga.calls), (150, 2), "summed across days");
        assert_eq!(ga.kind, "git_local");

        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_exec(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.cmd == "git add").unwrap().est_tokens, 100);

        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        assert!(ctx_exec(&conn, &f2).unwrap().is_empty());
    }

    #[test]
    fn context_totals_keep_unattributable_distinct_from_zero() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut claude = ev("a", "claude", DAY1_TS, "m", None, 1, 100, 50, 20, 0, 0);
        claude.ctx.messages = Some(1000);
        claude.ctx.system = Some(90);
        claude.ctx.toolcalls = Some(300);
        let hermes = ev("h", "hermes", DAY1_TS, "hermes-local", None, 1, 300, 100, 0, 0, 0);
        db::insert_events(&mut conn, &[claude, hermes]).unwrap();

        let c = context(&conn, &Filters::default()).unwrap();
        let cl = c.totals.iter().find(|t| t.source == "claude").unwrap();
        assert_eq!(cl.billed, 120);
        assert_eq!(cl.reused, 20);
        assert_eq!(cl.messages, Some(1000));
        assert_eq!(cl.system, Some(90));
        assert_eq!(cl.toolcalls, Some(300));
        assert_eq!(cl.agents, None, "a Source that cannot attribute a category yields no figure");
        let hm = c.totals.iter().find(|t| t.source == "hermes").unwrap();
        assert_eq!(hm.billed, 300);
        assert_eq!(hm.messages, None);
        assert_eq!(hm.toolcalls, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn reading(
        source: &str, window_key: &str, minutes: i64, used_pct: f64, resets_at: i64,
        observed_at: i64, via: &str, plan: Option<&str>,
    ) -> LimitReading {
        LimitReading {
            source: source.to_string(),
            window_key: window_key.to_string(),
            window_minutes: Some(minutes),
            used_pct,
            resets_at,
            observed_at,
            via: via.to_string(),
            plan: plan.map(str::to_string),
            provenance: ReadingProvenance::default(),
        }
    }

    /// A Reading proving everything, so a test can spoil exactly one fact.
    /// `via: "live"` because that is the only shape that carries an account in
    /// production — the Companion's; a rollout Reading never does (#183).
    fn proven_reading(used_pct: f64, observed_at: i64, resets_at: i64) -> LimitReading {
        LimitReading {
            source: "codex".to_string(),
            window_key: "w10080".to_string(),
            window_minutes: Some(10_080),
            used_pct,
            resets_at,
            observed_at,
            via: "live".to_string(),
            plan: Some("plus".to_string()),
            provenance: crate::types::ReadingProvenance {
                account_id: Some("acct-a".to_string()),
                metering_regime: Some("codex:rate_limits".to_string()),
                limit_id: Some("codex:w10080".to_string()),
                model_scope: Some(crate::types::ModelScope::All),
                source_order: Some(observed_at),
                covered_from: Some(0),
                external_activity: None,
            },
        }
    }

    /// The rollout's own shape: identity but no account or coverage — what the
    /// Codex scan stores for every request (#183).
    fn rollout_reading(used_pct: f64, observed_at: i64, resets_at: i64) -> LimitReading {
        let mut r = proven_reading(used_pct, observed_at, resets_at);
        r.via = "logs".to_string();
        r.provenance.account_id = None;
        r.provenance.covered_from = None;
        r
    }


    /// Usage Resets are current state from a live Source's Companion Export
    /// Artifact (glossary: Usage Reset), assembled onto the card by the query
    /// itself — the whole card is described in one place, reachable from this
    /// suite without an AppHandle.
    #[test]
    fn usage_resets_ride_the_card_from_the_live_exports() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_limit_readings(
            &mut conn,
            &[proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400)],
        )
        .unwrap();
        let exports = dir.path().join("limit-exports");
        crate::limits_artifact::write(
            &exports,
            &crate::limits_artifact::LimitsExport {
                source: "codex".to_string(),
                fetched_at: EVALUATED_AT - 60,
                usage_resets_available: Some(3),
                ..Default::default()
            },
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, &exports).unwrap();
        assert_eq!(cards[0].usage_resets_available, Some(3));
    }

    /// Catalog-driven, so no Source is named in the assembly: a second live
    /// Source's export fills ITS card — re-hardcoding `codex` fails this —
    /// and an unreported count stays unknown (None), never zero.
    #[test]
    fn usage_resets_follow_the_catalog_not_a_hardcoded_source() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut grok = proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400);
        grok.source = "grok".to_string();
        grok.provenance.metering_regime = Some("grok:rate_limits".to_string());
        grok.provenance.limit_id = Some("grok:w10080".to_string());
        let codex = proven_reading(35.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400);
        db::insert_limit_readings(&mut conn, &[grok, codex]).unwrap();
        let exports = dir.path().join("limit-exports");
        crate::limits_artifact::write(
            &exports,
            &crate::limits_artifact::LimitsExport {
                source: "grok".to_string(),
                fetched_at: EVALUATED_AT - 60,
                usage_resets_available: Some(2),
                ..Default::default()
            },
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, &exports).unwrap();
        let by = |key: &str| cards.iter().find(|c| c.source == key).unwrap();
        assert_eq!(by("grok").usage_resets_available, Some(2));
        assert_eq!(by("codex").usage_resets_available, None);
    }

    #[test]
    fn every_window_carries_exactly_one_evaluation_sharing_one_instant() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut five_hour = proven_reading(10.0, EVALUATED_AT - 600, EVALUATED_AT + 3_600);
        five_hour.window_key = "w300".to_string();
        five_hour.window_minutes = Some(300);
        five_hour.provenance.limit_id = Some("codex:w300".to_string());
        db::insert_limit_readings(
            &mut conn,
            &[proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400), five_hour],
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let windows = &cards[0].windows;
        assert_eq!(windows.len(), 2);
        for window in windows {
            // One evaluation each, and all of them answered as of one second.
            assert_eq!(window.estimate.evaluated_at, EVALUATED_AT);
            assert_eq!(window.estimate.policy_version, "limit-token-estimate-v1");
            // Nothing to be Ready on yet, so no figure — and never a zero.
            assert_eq!(window.estimate.outcome, LimitEstimateOutcome::Gathering);
        }
    }

    #[test]
    fn only_ready_puts_a_number_on_the_wire() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // A Reading whose window has already reset cannot anchor anything.
        db::insert_limit_readings(
            &mut conn,
            &[proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT - 60)],
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let estimate = &cards[0].windows[0].estimate;
        assert_eq!(estimate.outcome, LimitEstimateOutcome::Blocked);
        // Absent on the wire, not null: the shape a frontend narrows on.
        let json = serde_json::to_string(estimate).unwrap();
        assert!(!json.contains("tokensPerPct"), "{json}");
        assert!(json.contains("\"state\":\"blocked\""), "{json}");
        assert!(json.contains("\"no-current-reading\""), "{json}");
    }

    #[test]
    fn the_payload_stays_bounded_and_carries_no_record_identities() {
        use crate::types::{CtxTokens, UsageEvent};

        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();

        // Seven qualifying recent epochs — more than the payload may carry —
        // so the five-summary bound, the no-identities rule, and the
        // no-infinity rule are all tested against a payload that actually has
        // candidates to leak. The original fixture had zero, and every one of
        // these assertions passed vacuously.
        const DAY: i64 = 86_400;
        let mut readings = vec![proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + DAY)];
        let mut events = Vec::new();
        for (i, days_ago) in (1i64..=7).enumerate() {
            let ended = EVALUATED_AT - days_ago * DAY;
            for step in 0..3i64 {
                readings.push(proven_reading(
                    (step * 10) as f64,
                    ended - 7_200 + step * 3_600,
                    ended,
                ));
            }
            for step in 0..2i64 {
                events.push(UsageEvent {
                    dedup_key: format!("bounded-{i}-{step}"),
                    source: "codex".to_string(),
                    timestamp: ended - 7_200 + step * 3_600 + 1,
                    model: Some("gpt-5.4-codex".to_string()),
                    project: None,
                    api_calls: 1,
                    input_tokens: 1_000 + i as i64,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_5m_tokens: 0,
                    cache_write_1h_tokens: 0,
                    source_file: "bounded-rollout.jsonl".to_string(),
                    session_id: None,
                    reasoning_tokens: None,
                    ctx: CtxTokens::default(),
                });
            }
        }
        db::insert_limit_readings(&mut conn, &readings).unwrap();
        db::insert_events(&mut conn, &events).unwrap();
        conn.execute("UPDATE events SET account_id = 'acct-a'", []).unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let estimate = &cards[0].windows[0].estimate;
        assert_eq!(
            estimate.explanation.candidates.len(),
            5,
            "seven qualifying epochs, five summaries: the bound has to bite"
        );
        assert_eq!(estimate.explanation.required_epochs, 3);

        let json = serde_json::to_string(estimate).unwrap();
        // Rejections and candidates travel as counts and summaries, never as
        // the Records they counted — by value, so any spelling of a leaked
        // identity fails, not merely the Rust field name.
        assert!(!json.contains("bounded-"), "{json}");
        assert!(!json.contains("dedup_key") && !json.contains("sourceFile"), "{json}");
        // And an unbounded quantization upper is null, never an infinity.
        assert!(!json.contains("inf"), "{json}");
    }

    #[test]
    fn the_estimate_evaluates_from_the_newest_identity_bearing_reading() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // Codex's production timeline: the newest Reading overall is a rollout's
        // — account-less — while a recent Companion Reading proves the Series.
        // The evaluation must proceed from the live one rather than flapping to
        // missing-account-identity Blocked on every request (#183).
        db::insert_limit_readings(
            &mut conn,
            &[
                proven_reading(40.0, EVALUATED_AT - 600, EVALUATED_AT + 86_400),
                rollout_reading(41.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400),
            ],
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let window = &cards[0].windows[0];
        // The display stays the newest overall; only the estimate anchors on
        // the identity-bearing Reading.
        assert_eq!(window.used_pct, 41.0);
        assert_eq!(window.estimate.outcome, LimitEstimateOutcome::Gathering, "not Blocked");
        assert!(
            !window
                .estimate
                .explanation
                .reason_codes
                .contains(&ReasonCode::MissingAccountIdentity),
            "{:?}",
            window.estimate.explanation.reason_codes,
        );
    }

    #[test]
    fn a_windows_explanation_carries_what_its_own_evidence_refused() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // A Reading proving nothing, then one proving everything: the first is
        // refused for the fact it lacks, and that refusal is this window's.
        let mut unprovable = proven_reading(30.0, EVALUATED_AT - 900, EVALUATED_AT + 86_400);
        unprovable.provenance.account_id = None;
        db::insert_limit_readings(
            &mut conn,
            &[unprovable, proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400)],
        )
        .unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let rejections = &cards[0].windows[0].estimate.explanation.rejections;
        // An evidence-stage reason, not an estimator one: most of the
        // twenty-two live at that stage, and a page reporting only the
        // estimator's would explain almost nothing.
        assert!(
            rejections
                .iter()
                .any(|r| r.reason_code == ReasonCode::MissingAccountIdentity && r.count == 1),
            "{rejections:?}",
        );
    }

    #[test]
    fn a_stable_core_older_than_the_bounded_read_is_stale_not_gathering() {
        use crate::types::{CtxTokens, UsageEvent};

        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();

        // Three agreeing completed epochs around 100 days back — strictly older
        // than the two-horizon page-open read (84 days for a weekly window).
        // The fixed bound alone answered Gathering; the backwards paging must
        // walk on until it finds the Ready proof, and answer Stale (spec:
        // "Stop at the first Ready proof or when history is exhausted").
        const DAY: i64 = 86_400;
        let mut readings = vec![proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + DAY)];
        let mut events = Vec::new();
        for (i, days_ago) in [100i64, 101, 102].iter().enumerate() {
            let ended = EVALUATED_AT - days_ago * DAY;
            for step in 0..3i64 {
                readings.push(proven_reading(
                    (step * 10) as f64,
                    ended - 7_200 + step * 3_600,
                    ended,
                ));
            }
            for step in 0..2i64 {
                events.push(UsageEvent {
                    dedup_key: format!("stale-{i}-{step}"),
                    source: "codex".to_string(),
                    timestamp: ended - 7_200 + step * 3_600 + 1,
                    model: Some("gpt-5.4-codex".to_string()),
                    project: None,
                    api_calls: 1,
                    input_tokens: 1_000 + i as i64,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_5m_tokens: 0,
                    cache_write_1h_tokens: 0,
                    source_file: "rollout.jsonl".to_string(),
                    session_id: None,
                    reasoning_tokens: None,
                    ctx: CtxTokens::default(),
                });
            }
        }
        db::insert_limit_readings(&mut conn, &readings).unwrap();
        db::insert_events(&mut conn, &events).unwrap();
        // `account_id` sits outside db::COLS, so no insert path writes it (#171).
        conn.execute("UPDATE events SET account_id = 'acct-a'", []).unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        let estimate = &cards[0].windows[0].estimate;
        assert_eq!(estimate.outcome, LimitEstimateOutcome::Stale, "not Gathering");
        assert!(
            estimate.explanation.reason_codes.contains(&ReasonCode::HistoricalCoreAgedOut),
            "{:?}",
            estimate.explanation.reason_codes,
        );
    }

    #[test]
    fn epoch_summaries_carry_their_reason_codes_and_never_their_models() {
        use crate::limits_evidence::{Interval, PartitionEvidence};

        // Three agreeing epochs and one outlier, evaluated and put on the wire.
        // The outlier's summary says why it sits outside the core, in codes; a
        // core member says nothing. And the raw Model composition the internal
        // candidates retain stays off the wire — the public shape has no
        // models field (spec: "Public shape").
        let epoch = |days_ago: i64, tokens: i64| {
            let ended = EVALUATED_AT - days_ago * 86_400;
            let interval = |from_pct: i64, tokens: i64, t0: i64| Interval {
                from_pct,
                to_pct: from_pct + 10,
                tokens,
                t0,
                t1: t0 + 3_600,
                models: std::iter::once("gpt-5.4-codex".to_string()).collect(),
            };
            PartitionEvidence {
                series: SeriesKey {
                    source: "codex".to_string(),
                    account_id: "acct-a".to_string(),
                    plan: "plus".to_string(),
                    metering_regime: "codex:rate_limits".to_string(),
                    limit_id: "codex:w10080".to_string(),
                    model_scope: "all".to_string(),
                },
                epoch: ended,
                window_minutes: Some(10_080),
                intervals: vec![
                    interval(0, tokens / 2, ended - 7_200),
                    interval(10, tokens - tokens / 2, ended - 3_600),
                ],
            }
        };
        let partitions = vec![
            epoch(1, 2_000),
            epoch(2, 2_020),
            epoch(3, 1_980),
            epoch(4, 8_000),
        ];
        let current = proven_reading(40.0, EVALUATED_AT - 300, EVALUATED_AT + 86_400);
        let evaluation = limits_readiness::evaluate(Some(&current), &partitions, EVALUATED_AT);
        let wire = on_the_wire(evaluation, Some(&current), BTreeMap::new()).unwrap();

        assert!(matches!(wire.outcome, LimitEstimateOutcome::Ready { .. }));
        let summaries = &wire.explanation.candidates;
        assert_eq!(summaries.len(), 4);
        for member in summaries.iter().filter(|s| s.in_core) {
            assert_eq!(member.reason_codes, vec![], "a core member has nothing to answer for");
        }
        let outlier = summaries.iter().find(|s| !s.in_core).expect("one epoch is left out");
        assert_eq!(outlier.reason_codes, vec![ReasonCode::QuantizationRangesDisjoint]);

        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains("\"state\":\"ready\""), "{json}");
        assert!(json.contains("\"tokensPerPct\":"), "{json}");
        assert!(json.contains("\"reasonCodes\""), "{json}");
        assert!(!json.contains("models") && !json.contains("gpt-5.4-codex"), "{json}");
    }

    #[test]
    fn an_epoch_key_tells_epochs_apart_without_telling_on_the_account() {
        let series = SeriesKey {
            source: "codex".to_string(),
            account_id: "acct-secret".to_string(),
            plan: "plus".to_string(),
            metering_regime: "codex:rate_limits".to_string(),
            limit_id: "codex:w10080".to_string(),
            model_scope: "all".to_string(),
        };
        let first = epoch_key(&series, 1_000);
        assert_eq!(first, epoch_key(&series, 1_000), "the same epoch keys the same");
        assert_ne!(first, epoch_key(&series, 2_000), "a later epoch keys differently");

        let mut elsewhere = series.clone();
        elsewhere.account_id = "acct-other".to_string();
        assert_ne!(first, epoch_key(&elsewhere, 1_000), "another account, another key");
        assert!(!first.contains("acct"), "and the account is not in it: {first}");
    }

    #[test]
    fn limits_takes_the_highest_percentage_of_the_newest_epoch() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_limit_readings(&mut conn, &[
            // A finished epoch: its fill-curve stays in the table but must not
            // be what the card shows.
            reading("codex", "w10080", 10080, 90.0, 1_786_000_000, 1_785_900_000, "logs", Some("plus")),
            // The newest epoch, with its curve out of order and jittered stamps —
            // 1s of wobble is the same window, not a newer one (#104).
            reading("codex", "w10080", 10080, 41.0, 1_786_879_486, 1_786_331_779, "logs", Some("plus")),
            reading("codex", "w10080", 10080, 44.0, 1_786_879_485, 1_786_331_900, "logs", Some("plus")),
            reading("codex", "w10080", 10080, 40.0, 1_786_879_487, 1_786_331_700, "logs", Some("plus")),
            // A second window of the same Source, and a second Source.
            reading("codex", "w300", 300, 12.0, 1_786_350_000, 1_786_331_900, "logs", Some("plus")),
            reading("claude", "five_hour", 300, 18.0, 1_786_350_000, 1_786_340_000, "live", Some("Team 5x")),
        ]).unwrap();

        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        assert_eq!(cards.len(), 2, "one card per Source holding Readings");

        let claude = &cards[0];
        assert_eq!(claude.source, "claude");
        assert_eq!(claude.plan.as_deref(), Some("Team 5x"));
        assert_eq!(claude.windows.len(), 1);

        let codex = &cards[1];
        assert_eq!(codex.plan.as_deref(), Some("plus"));
        // Ordered by duration, so the session window precedes the weekly one.
        assert_eq!(
            codex.windows.iter().map(|w| w.window_key.as_str()).collect::<Vec<_>>(),
            vec!["w300", "w10080"],
        );
        let weekly = &codex.windows[1];
        assert_eq!(weekly.used_pct, 44.0, "the newest epoch's highest fill, not the older 90%");
        assert_eq!(weekly.resets_at, 1_786_879_487, "the epoch's own reset instant");
        assert_eq!(weekly.observed_at, 1_786_331_900, "and its newest observation");
    }

    #[test]
    fn limits_is_empty_and_plan_free_without_readings() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        assert_eq!(limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap(), vec![]);

        // A Source that only ever reported a null plan still gets its card.
        db::insert_limit_readings(&mut conn, &[
            reading("codex", "w10080", 10080, 5.0, 1_786_879_486, 1_786_331_779, "logs", None),
        ]).unwrap();
        let cards = limits(&conn, EVALUATED_AT, std::path::Path::new("")).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].plan, None, "an absent plan is unknown, never guessed");
    }
}
