use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    pub tools: Vec<String>,
    pub models: Vec<String>,
    pub project: Option<String>,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub requests: i64,
    pub cost: Option<f64>,
    pub has_unpriced: bool,
    pub unpriced_models: Vec<String>,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub bucket: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub requests: i64,
    pub cost: Option<f64>,
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
                cost += in_ as f64 * rt.input
                    + out as f64 * rt.output
                    + cr as f64 * rt.cache_read
                    + w5 as f64 * rt.cache_write_5m
                    + w1 as f64 * rt.cache_write_1h;
                priced_tokens += tokens;
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
        unpriced_models,
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
        let c = match rates.resolve(&model) {
            Some(rt) => {
                in_ as f64 * rt.input
                    + out as f64 * rt.output
                    + cr as f64 * rt.cache_read
                    + w5 as f64 * rt.cache_write_5m
                    + w1 as f64 * rt.cache_write_1h
            }
            None => 0.0,
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
            });
            points.len() - 1
        });
        let p = &mut points[i];
        p.input_tokens += in_;
        p.output_tokens += out;
        p.cache_read_tokens += cr;
        p.cache_write_tokens += w5 + w1;
        p.total_tokens += in_ + out + cr + w5 + w1;
        p.cost += c;
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
}

pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>> {
    let group_col = match by {
        "tool" => "source",
        "project" => "project",
        _ => "model",
    };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT {group_col} AS grp, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls) \
         FROM events {where_sql} GROUP BY grp, model"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
        ))
    })?;

    let mut map: HashMap<String, Agg> = HashMap::new();
    for row in rows {
        let (grp, model, in_, out, cr, w5, w1, calls) = row?;
        let key = grp.unwrap_or_else(|| "unknown".to_string());
        let a = map.entry(key).or_default();
        a.input += in_;
        a.output += out;
        a.cache_read += cr;
        a.cache_write += w5 + w1;
        a.total += in_ + out + cr + w5 + w1;
        a.requests += calls;
        if let Some(rt) = rates.resolve(&model) {
            a.cost += in_ as f64 * rt.input
                + out as f64 * rt.output
                + cr as f64 * rt.cache_read
                + w5 as f64 * rt.cache_write_5m
                + w1 as f64 * rt.cache_write_1h;
            a.priced += in_ + out + cr + w5 + w1;
        }
    }

    let mut out: Vec<BreakdownRow> = map
        .into_iter()
        .map(|(key, a)| BreakdownRow {
            key,
            input_tokens: a.input,
            output_tokens: a.output,
            cache_read_tokens: a.cache_read,
            cache_write_tokens: a.cache_write,
            total_tokens: a.total,
            requests: a.requests,
            cost: if a.priced > 0 { Some(a.cost) } else { None },
        })
        .collect();
    out.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    Ok(out)
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
        assert_eq!(rows[0].key, "gpt-5.4");
        assert_eq!(rows[0].total_tokens, 4850);
        assert_eq!(rows[0].requests, 2);
        approx(rows[0].cost.unwrap(), 0.02155);
        assert_eq!(rows[1].key, "hermes-local");
        assert_eq!(rows[1].total_tokens, 400);
        assert_eq!(rows[1].requests, 3);
        assert!(rows[1].cost.is_none());
    }

    #[test]
    fn breakdown_by_project_maps_null_to_unknown() {
        let (_dir, conn) = seed();
        let rows = breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(rows[0].key, "/Users/dev/projects/alpha");
        assert_eq!(rows[0].total_tokens, 4850);
        assert_eq!(rows[1].key, "unknown");
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
        assert_eq!(pts[1].bucket, "2026-07-02");
        assert_eq!(pts[1].total_tokens, 3000); // B
        approx(pts[1].cost, 0.014);
    }
}
