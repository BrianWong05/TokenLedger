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
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct TrendPoint {
    pub bucket: String,
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
    pub cost: f64,
    pub has_unpriced: bool,
    #[ts(type = "number")]
    pub unattributed_tokens: i64,
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

use std::collections::HashMap;
use rusqlite::{params_from_iter, types::Value, Connection};
use crate::pricing::RateMap;

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

pub fn summary(conn: &Connection, f: &Filters) -> rusqlite::Result<Summary> {
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls) \
         FROM events {where_sql} GROUP BY model"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
        ))
    })?;

    let (mut input, mut output, mut cache_read, mut cw5m, mut cw1h, mut requests) =
        (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut cost = 0.0f64;
    let mut priced_tokens = 0i64;
    let mut unpriced_models: Vec<String> = Vec::new();
    let mut cache_estimated_models: Vec<String> = Vec::new();

    for row in rows {
        let (model, in_, out, cr, w5, w1, calls) = row?;
        input += in_;
        output += out;
        cache_read += cr;
        cw5m += w5;
        cw1h += w1;
        requests += calls;
        let tokens = in_ + out + cr + w5 + w1;
        match rates.resolve(&model) {
            Some(rt) => {
                cost += rt.cost(in_, out, cr, w5, w1);
                priced_tokens += tokens;
                if rt.cache_gap(cr, w5, w1) {
                    cache_estimated_models.push(model);
                }
            }
            None => {
                if tokens > 0 {
                    unpriced_models.push(model);
                }
            }
        }
    }

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
        cost: if priced_tokens > 0 { Some(cost) } else { None },
        has_unpriced: !unpriced_models.is_empty(),
        unattributed_tokens: 0,
        unpriced_models,
        cache_estimated_models,
        cache_hit_rate,
    })
}

pub fn trend(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<TrendPoint>> {
    let fmt = if bucket == "hour" { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens) \
         FROM events {where_sql} GROUP BY bucket, model ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
        ))
    })?;

    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut points: Vec<TrendPoint> = Vec::new();
    for row in rows {
        let (bucket, model, in_, out, cr, w5, w1) = row?;
        let tokens = in_ + out + cr + w5 + w1;
        let (c, unpriced) = match rates.resolve(&model) {
            Some(rt) => (rt.cost(in_, out, cr, w5, w1), false),
            None => (0.0, tokens > 0),
        };
        let i = *idx.entry(bucket.clone()).or_insert_with(|| {
            points.push(TrendPoint {
                bucket: bucket.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                total_tokens: 0,
                cost: 0.0,
                has_unpriced: false,
                unattributed_tokens: 0,
            });
            points.len() - 1
        });
        let p = &mut points[i];
        p.input_tokens += in_;
        p.output_tokens += out;
        p.cache_read_tokens += cr;
        p.cache_write_tokens += w5 + w1;
        p.total_tokens += tokens;
        p.cost += c;
        p.has_unpriced |= unpriced;
    }
    Ok(points)
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
    #[ts(type = "number | null")]
    pub ctx_messages: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_system: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_reasoning: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_toolcalls: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_agents: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_mcp: Option<i64>,
    #[ts(type = "number | null")]
    pub ctx_skills: Option<i64>,
}

// Merges a nullable per-group SUM into an accumulator: only Some contributes,
// so a group whose values are all NULL stays None (never coerced to 0).
fn add_opt(acc: &mut Option<i64>, v: Option<i64>) {
    if let Some(x) = v {
        *acc = Some(acc.unwrap_or(0) + x);
    }
}

// Per-(bucket, source) series — the real-data twin of the frontend mock's DAYS.
pub fn series(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<SeriesPoint>> {
    let fmt = if bucket == "hour" { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);

    // Tokens/cost need per-model rows for rate resolution.
    let sql = format!(
        "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, source, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens), \
         SUM(ctx_messages), SUM(ctx_system), SUM(ctx_reasoning), SUM(ctx_toolcalls), SUM(ctx_agents), SUM(ctx_mcp), SUM(ctx_skills) \
         FROM events {where_sql} GROUP BY bucket, source, model ORDER BY bucket, source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?,
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, Option<i64>>(10)?, r.get::<_, Option<i64>>(11)?, r.get::<_, Option<i64>>(12)?,
            r.get::<_, Option<i64>>(13)?, r.get::<_, Option<i64>>(14)?, r.get::<_, Option<i64>>(15)?,
            r.get::<_, Option<i64>>(16)?,
        ))
    })?;

    let mut idx: HashMap<(String, String), usize> = HashMap::new();
    let mut points: Vec<SeriesPoint> = Vec::new();
    for row in rows {
        let (bucket, source, model, in_, out, cr, w5, w1, calls, reasoning,
             cxm, cxs, cxr, cxt, cxa, cxmc, cxsk) = row?;
        let tokens = in_ + out + cr + w5 + w1;
        let (c, unpriced) = match rates.resolve(&model) {
            Some(rt) => (rt.cost(in_, out, cr, w5, w1), false),
            None => (0.0, tokens > 0),
        };
        // Clone into the map key like trend(); avoid moving (bucket, source) before push.
        let i = *idx.entry((bucket.clone(), source.clone())).or_insert_with(|| {
            points.push(SeriesPoint {
                bucket: bucket.clone(),
                source: source.clone(),
                by_model: HashMap::new(),
                unattributed_tokens: 0,
                has_unpriced: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                total_tokens: 0,
                reasoning_tokens: None,
                cost: 0.0,
                requests: 0,
                convs: 0,
                ctx_messages: None,
                ctx_system: None,
                ctx_reasoning: None,
                ctx_toolcalls: None,
                ctx_agents: None,
                ctx_mcp: None,
                ctx_skills: None,
            });
            points.len() - 1
        });
        let p = &mut points[i];
        *p.by_model.entry(model).or_insert(0) += tokens;
        p.has_unpriced |= unpriced;
        p.input_tokens += in_;
        p.output_tokens += out;
        p.cache_read_tokens += cr;
        p.cache_write_tokens += w5 + w1;
        p.total_tokens += in_ + out + cr + w5 + w1;
        p.requests += calls;
        p.cost += c;
        add_opt(&mut p.reasoning_tokens, reasoning);
        add_opt(&mut p.ctx_messages, cxm);
        add_opt(&mut p.ctx_system, cxs);
        add_opt(&mut p.ctx_reasoning, cxr);
        add_opt(&mut p.ctx_toolcalls, cxt);
        add_opt(&mut p.ctx_agents, cxa);
        add_opt(&mut p.ctx_mcp, cxmc);
        add_opt(&mut p.ctx_skills, cxsk);
    }

    // Convs need distinct-count at (bucket, source) — a session can span
    // models, so distinct-per-model counts cannot be summed.
    let sql2 = format!(
        "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, source, \
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
    let sql = format!(
        "SELECT {group_col} AS grp, {src_expr} AS src, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens) \
         FROM events {where_sql} GROUP BY grp, src, model"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?,
            r.get::<_, Option<i64>>(9)?,
        ))
    })?;

    let mut map: HashMap<(String, Option<String>), Agg> = HashMap::new();
    for row in rows {
        let (grp, src, model, in_, out, cr, w5, w1, calls, reasoning) = row?;
        let key = (grp.unwrap_or_else(|| "unknown".to_string()), src);
        let a = map.entry(key).or_default();
        a.input += in_;
        a.output += out;
        a.cache_read += cr;
        a.cache_write += w5 + w1;
        a.total += in_ + out + cr + w5 + w1;
        a.requests += calls;
        if let Some(r) = reasoning {
            a.reasoning = Some(a.reasoning.unwrap_or(0) + r);
        }
        if let Some(rt) = rates.resolve(&model) {
            a.cost += rt.cost(in_, out, cr, w5, w1);
            a.priced += in_ + out + cr + w5 + w1;
            a.cache_estimated |= rt.cache_gap(cr, w5, w1);
        } else {
            a.unpriced = true;
        }
    }

    // Convs at the row's own grain (distinct sessions can span models).
    let sql2 = format!(
        "SELECT {group_col} AS grp, {src_expr} AS src, COUNT(DISTINCT session_id) \
         FROM events {where_sql} GROUP BY grp, src"
    );
    let mut stmt2 = conn.prepare(&sql2)?;
    let crows = stmt2.query_map(params_from_iter(params.iter()), |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in crows {
        let (grp, src, convs) = row?;
        let key = (grp.unwrap_or_else(|| "unknown".to_string()), src);
        if let Some(a) = map.get_mut(&key) {
            a.convs = convs;
        }
    }

    let mut out: Vec<BreakdownRow> = map
        .into_iter()
        .map(|((key, source), a)| BreakdownRow {
            key: Some(key),
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
            unattributed_tokens: 0,
        })
        .collect();
    out.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    Ok(out)
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct CtxResourceCount {
    pub source: String,
    pub kind: String,
    #[ts(type = "number")]
    pub count: i64,
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

// Distinct resources (skills / MCP servers / agents / memory files) seen in
// range, per source — the Context Breakdown meta line.
pub fn ctx_resources(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxResourceCount>> {
    let (where_sql, params) = day_where(f);
    let sql = format!(
        "SELECT source, kind, COUNT(DISTINCT name) FROM ctx_resources {where_sql} \
         GROUP BY source, kind"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxResourceCount { source: r.get(0)?, kind: r.get(1)?, count: r.get(2)? })
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
pub fn ctx_buckets(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxBuckets>> {
    // A Hermes Usage Record is Session-granularity — one Record stands for a
    // whole Session of calls — so "first cache-write per session = System
    // prompt" cannot apply to it; its writes count as history, not System.
    const FIRST_CW_IS_SYSTEM: &str =
        "cw_rank = 1 AND session_id IS NOT NULL AND source != 'hermes'";
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "WITH ranked AS ( \
           SELECT source, model, project, timestamp, session_id, \
                  input_tokens, output_tokens, cache_read_tokens, reasoning_tokens, \
                  cache_write_5m_tokens + cache_write_1h_tokens AS cw, \
                  ROW_NUMBER() OVER ( \
                    PARTITION BY source, session_id, \
                      CASE WHEN cache_write_5m_tokens + cache_write_1h_tokens > 0 THEN 1 ELSE 0 END \
                    ORDER BY timestamp, dedup_key) AS cw_rank \
           FROM events) \
         SELECT source, \
           SUM(cache_read_tokens) + SUM(CASE WHEN cw > 0 AND NOT ({FIRST_CW_IS_SYSTEM}) THEN cw ELSE 0 END), \
           SUM(input_tokens), \
           SUM(CASE WHEN cw > 0 AND {FIRST_CW_IS_SYSTEM} THEN cw END), \
           SUM(output_tokens), \
           SUM(reasoning_tokens) \
         FROM ranked {where_sql} GROUP BY source ORDER BY source"
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
pub fn ctx_tools(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxToolRow>> {
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
pub fn ctx_exec(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxExecRow>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::pricing::{self, OverrideRates};
    use crate::types::UsageEvent;
    use tempfile::tempdir;

    // 2026-07-01T12:00:00Z and 2026-07-02T12:00:00Z (event times)
    const DAY1_TS: i64 = 1_782_907_200;
    const DAY2_TS: i64 = 1_782_993_600;
    // 2026-07-01T00:00:00Z and 2026-07-02T00:00:00Z (local-midnight bounds under TZ=UTC)
    const DAY1_START: i64 = 1_782_864_000;
    const DAY2_START: i64 = 1_782_950_400;

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
            model: model.to_string(),
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
    fn trend_daily_buckets_local_time() {
        std::env::set_var("TZ", "UTC"); // pin bucketing timezone for a deterministic date string
        let (_dir, conn) = seed();
        let pts = trend(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].bucket, "2026-07-01");
        assert_eq!(pts[0].total_tokens, 2250); // A + C
        approx(pts[0].cost, 0.00755);
        assert!(pts[0].has_unpriced);
        assert_eq!(pts[0].unattributed_tokens, 0);
        assert_eq!(pts[1].bucket, "2026-07-02");
        assert_eq!(pts[1].total_tokens, 3000); // B
        approx(pts[1].cost, 0.014);
        assert!(!pts[1].has_unpriced);
        assert_eq!(pts[1].unattributed_tokens, 0);
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
    fn series_sums_ctx_preserving_null() {
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

        let pts = series(&conn, &Filters::default(), "day").unwrap();
        let c = pts.iter().find(|p| p.source == "claude").unwrap();
        assert_eq!(c.ctx_messages, Some(1000));
        assert_eq!(c.ctx_system, Some(90));
        assert_eq!(c.ctx_reasoning, Some(20));
        assert_eq!(c.ctx_toolcalls, Some(300));
        assert_eq!(c.ctx_agents, None, "no contributing value: NULL, never 0");
        let hm = pts.iter().find(|p| p.source == "hermes").unwrap();
        assert_eq!(hm.ctx_messages, None);
    }

    #[test]
    fn ctx_resources_counts_distinct_in_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("t.db")).unwrap();
        crate::db::record_resources(&conn, "claude", &[
            ("skill", "graphify".to_string(), DAY1_TS),
            ("skill", "graphify".to_string(), DAY2_TS), // same name, new day: still 1 distinct
            ("skill", "verify".to_string(), DAY2_TS),
            ("mcp_server", "pencil".to_string(), DAY1_TS),
        ]).unwrap();

        let all = ctx_resources(&conn, &Filters::default()).unwrap();
        let skill = all.iter().find(|r| r.kind == "skill").unwrap();
        assert_eq!(skill.count, 2);
        let mcp = all.iter().find(|r| r.kind == "mcp_server").unwrap();
        assert_eq!(mcp.count, 1);

        // Day-1-only window excludes the day-2 'verify'.
        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_resources(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.kind == "skill").unwrap().count, 1);

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
        db::insert_events(&mut conn, &[a, b, c, h]).unwrap();

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
}
