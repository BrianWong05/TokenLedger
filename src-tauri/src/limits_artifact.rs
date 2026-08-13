// The Limits Export Artifact — the one contract between the Companions, which
// write these files, and the scan, which reads them like any other file
// (ADR-0019). Both sides use these types, so a field can never be spelled one
// way by the writer and another by the reader.
//
// One Artifact per `live` Source, named `<source>.tokenledger-limits.json`, in a
// directory the app owns. It carries Limit Readings — never tokens, never usage —
// which is the whole of what the Companion is allowed to learn.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::adapters::unchanged;
use crate::db::{self, set_file_state};
use crate::types::{FileState, LimitReading, ReadingProvenance};

pub const SUFFIX: &str = ".tokenledger-limits.json";

/// The one failure prefix the app classifies as an absence rather than an error
/// (its regex also matches 401/403). Every Companion reports a missing or
/// refused credential behind this prefix; anything else is a failure and must
/// never borrow it — telling someone they are not signed in when the credential
/// store was merely unreadable sends them to re-authenticate a login they
/// already have.
pub const NOT_SIGNED_IN: &str = "not signed in";

/// The durations Codex's own labeller recognises, matched within ±5% because
/// upstream rounding drifts (#104). An unrecognised duration keeps its raw
/// minutes rather than being treated as corrupt.
const CANONICAL_WINDOW_MINUTES: [i64; 5] = [300, 1440, 10080, 43200, 525600];

/// Duration → the opaque `window_key` a Reading is stored under (`w300`,
/// `w10080`). One grammar for every producer — the scan's log ingest and a
/// Companion's live fetch must key the same window identically, or the two
/// halves of one series would render as two bars.
pub fn window_key(window_minutes: i64) -> String {
    let canonical = CANONICAL_WINDOW_MINUTES
        .iter()
        .find(|&&m| (window_minutes - m).abs() * 20 <= m)
        .copied()
        .unwrap_or(window_minutes);
    format!("w{canonical}")
}

/// Grok's billing `config` object → the one credit window it describes, shared by
/// the two producers that see the identical shape: the scan's log ingest (the
/// `ctx.config` of a `billing: fetched credits config` line) and the live
/// Companion (the `config` of a `/v1/billing?format=credits` response). Keeping
/// one mapper means a new period type or a changed field is edited once, not in
/// two files that would silently drift.
///
/// The window is keyed off the vendor's own period *type*, never the measured
/// duration — a 28-day February falls outside the canonical 43200 ±5% band, so
/// classifying by duration would split one card's history into two keys once a
/// year — and through the shared `window_key` grammar, so a live reading and a
/// logged one of the same window land in the same series. An absent
/// `creditUsagePercent` is 0% used: the payload is proto3-as-JSON, which omits
/// zero-valued scalars, so dropping it would lose the start of every window.
pub fn grok_credit_window(config: &serde_json::Value) -> Option<WindowExport> {
    let period = config.get("currentPeriod");
    let used_pct = config
        .get("creditUsagePercent")
        .and_then(|p| p.as_f64())
        .unwrap_or(0.0);
    // `billingPeriodEnd` is the deprecated mirror, identical on every observed
    // row; a payload carrying only it names no period type, and an unnameable
    // window cannot be keyed however well its reset is known.
    let resets_at = period
        .and_then(|p| p.get("end"))
        .or_else(|| config.get("billingPeriodEnd"))
        .and_then(|e| e.as_str())
        .and_then(crate::time::iso_to_epoch)?;
    let canonical_minutes = match period.and_then(|p| p.get("type")).and_then(|t| t.as_str())? {
        "USAGE_PERIOD_TYPE_WEEKLY" => 10_080,
        "USAGE_PERIOD_TYPE_MONTHLY" => 43_200,
        // A period type nobody has seen is not guessed into a lane it may not
        // belong to; an absent window is unknown, never zero.
        _ => return None,
    };
    // The bar's time axis, measured where the payload states both bounds — a
    // calendar month is not 43200 minutes, and the tick would sit wrong.
    let window_minutes = period
        .and_then(|p| p.get("start"))
        .and_then(|s| s.as_str())
        .and_then(crate::time::iso_to_epoch)
        .map(|start| (resets_at - start) / 60)
        .filter(|&m| m > 0)
        .unwrap_or(canonical_minutes);
    Some(WindowExport {
        key: window_key(canonical_minutes),
        window_minutes: Some(window_minutes),
        used_pct,
        resets_at,
    })
}

/// Bump when the shape changes. An Artifact declaring a schema the reader does
/// not know is a malformed instance of a supported shape (ADR-0015): it warns
/// and is not read, rather than being guessed at.
pub const SCHEMA: u32 = 2;

fn supported_schema(schema: u32) -> bool {
    schema == 1 || schema == SCHEMA
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsExport {
    pub schema: u32,
    /// The Source these Readings describe — the catalog key, e.g. `claude`.
    pub source: String,
    /// When the Companion asked the vendor, epoch seconds. This is the Readings'
    /// `observed_at`: a live figure is only as fresh as the fetch behind it.
    pub fetched_at: i64,
    /// The plan label the credential document carried (`rateLimitTier`).
    #[serde(default)]
    pub plan: Option<String>,
    /// Codex Usage Resets currently available. This is source-level current
    /// state, not a rolling-window Reading and not history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_resets_available: Option<u64>,
    #[serde(default)]
    pub windows: Vec<WindowExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowExport {
    /// How the Reading's window is addressed: the vendor's own response key
    /// where there is one (`five_hour`, `seven_day_opus`, or one nobody has seen
    /// yet), or a key this side builds where the vendor's does not identify a
    /// window uniquely — `w{minutes}` from a duration, and a `pool:` prefix
    /// where a Source meters more than one pool over the same durations. The
    /// page splits on the colon and classifies the remainder; nothing else
    /// reads inside it.
    pub key: String,
    /// The window's length, where the key names one. Absent means unknown — the
    /// card then draws a bar with no time tick rather than inventing an axis.
    #[serde(default)]
    pub window_minutes: Option<i64>,
    /// The vendor's own utilization figure, unconverted.
    pub used_pct: f64,
    /// Unix seconds. The vendor's ISO-8601 stamp is converted by the Companion,
    /// so the reader never has to guess at a format.
    pub resets_at: i64,
}

/// The Artifact that carries one Source's live Limits.
pub fn file_name(source: &str) -> String {
    format!("{source}{SUFFIX}")
}

/// `<source>.tokenledger-limits.json` → `<source>`; any other name → None.
/// The staging name a rename-write passes through must never be picked up.
pub fn source_key(path: &Path) -> Option<String> {
    path.file_name()?.to_str()?.strip_suffix(SUFFIX).map(str::to_string)
}

/// Every Reading in one export, stamped `via = 'live'`.
pub fn readings(export: &LimitsExport) -> Vec<LimitReading> {
    export
        .windows
        .iter()
        .map(|w| LimitReading {
            source: export.source.clone(),
            window_key: w.key.clone(),
            window_minutes: w.window_minutes,
            used_pct: w.used_pct,
            resets_at: w.resets_at,
            observed_at: export.fetched_at,
            via: "live".to_string(),
            plan: export.plan.clone(),
            provenance: ReadingProvenance::default(),
        })
        .collect()
}

pub fn path_in(dir: &Path, source: &str) -> PathBuf {
    dir.join(file_name(source))
}

/// Read the current source-level state from an Artifact. Invalid, stale-schema,
/// or mismatched files simply report no current state; `ingest` remains the
/// warning path for malformed exports.
pub fn read(dir: &Path, source: &str) -> Option<LimitsExport> {
    let raw = std::fs::read_to_string(path_in(dir, source)).ok()?;
    let export = serde_json::from_str::<LimitsExport>(&raw).ok()?;
    (supported_schema(export.schema) && export.source == source).then_some(export)
}

/// A vendor response's structure, values mostly redacted — every Companion's
/// `--shape` diagnostic. Keys, numbers, booleans, and short enum-ish strings
/// print; anything longer (tokens, uuids, prose) reduces to its length, so a
/// drifted payload can be diagnosed from a pasted transcript without usage
/// identifiers in it.
pub fn shape(node: &serde_json::Value) -> String {
    fn walk(node: &serde_json::Value, path: &str, out: &mut String) {
        use serde_json::Value;
        match node {
            Value::Object(object) => {
                for (key, value) in object {
                    walk(value, &format!("{path}.{key}"), out);
                }
            }
            Value::Array(items) => {
                out.push_str(&format!("{path}: [{} items]\n", items.len()));
                for (i, value) in items.iter().take(3).enumerate() {
                    walk(value, &format!("{path}[{i}]"), out);
                }
            }
            Value::String(s) if s.len() > 24 => out.push_str(&format!("{path}: <str {}>\n", s.len())),
            other => out.push_str(&format!("{path}: {other}\n")),
        }
    }
    let mut out = String::new();
    walk(node, "", &mut out);
    out
}

/// Rename-write one Source's export (ADR-0018): a reader never sees half a
/// document, and a crash mid-write leaves the previous Artifact intact. Shared
/// by every Companion, so the write discipline cannot drift between them.
pub fn write(dir: &Path, export: &LimitsExport) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let final_path = path_in(dir, &export.source);
    let staging = final_path.with_extension("json.part");
    {
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(serde_json::to_string(export)?.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&staging, &final_path)
}

/// Read one Source's Limits export out of `dir` and append its Readings.
/// Absent file → nothing to do, and not an error: a Source whose Companion has
/// never run is not a Source in trouble.
///
/// Idempotent twice over — the file's own state gates the re-read, and the
/// export carries its own `fetched_at`, so a second read of one export lands on
/// the Reading already stored — and both the scan and the command that just ran
/// the Companion can call this freely.
pub fn ingest(conn: &mut Connection, dir: &Path, source: &str) -> Result<(), String> {
    let path = path_in(dir, source);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => return Ok(()),
    };
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let state = FileState {
        size: meta.len() as i64,
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        byte_offset: 0,
    };
    if unchanged(conn, &path, &state) {
        return Ok(());
    }

    let export: LimitsExport = match serde_json::from_str::<LimitsExport>(&raw) {
        Ok(export) if supported_schema(export.schema) => export,
        // A malformed instance of a *supported* shape: warn, per ADR-0015. File
        // state stays unwritten so a corrected export is read on the next pass.
        _ => {
            return Err(format!(
                "unreadable limits export {}",
                path.to_string_lossy()
            ))
        }
    };
    // An export naming a different Source would file its Readings under the
    // wrong card, so the name on the tin has to match what is in it.
    if export.source != source {
        return Err(format!(
            "limits export {} describes {} rather than {source}",
            path.to_string_lossy(),
            export.source
        ));
    }

    db::insert_limit_readings(conn, &readings(&export)).map_err(|e| e.to_string())?;
    set_file_state(conn, &path.to_string_lossy(), state).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use crate::time::iso_to_epoch;
    use serde_json::Value;

    fn write_file(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    const EXPORT: &str = r#"{"schema":1,"source":"claude","fetched_at":1786492800,
      "plan":"Team 5x","windows":[
        {"key":"five_hour","window_minutes":300,"used_pct":18.0,"resets_at":1786503900},
        {"key":"seven_day_zephyr","window_minutes":10080,"used_pct":37.0,"resets_at":1786838400},
        {"key":"monthly_experiment","used_pct":5.0,"resets_at":1786838400}]}"#;

    #[test]
    fn the_name_a_writer_produces_is_the_name_a_reader_recognises() {
        let named = PathBuf::from("/tmp").join(file_name("claude"));
        assert_eq!(named.file_name().unwrap(), "claude.tokenledger-limits.json");
        assert_eq!(source_key(&named).as_deref(), Some("claude"));
        // The staging name a rename-write passes through is not an export.
        assert_eq!(source_key(Path::new("/tmp/claude.tokenledger-limits.json.part")), None);
        assert_eq!(source_key(Path::new("/tmp/claude.json")), None);
    }

    #[test]
    fn an_export_parses_into_live_readings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        write_file(&dir, &file_name("claude"), EXPORT);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        assert_eq!(ingest(&mut conn, &dir, "claude"), Ok(()));
        let rows: Vec<(String, Option<i64>, f64, i64, i64, String, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT window_key, window_minutes, used_pct, resets_at, observed_at, via, plan \
                     FROM limit_readings WHERE source = 'claude' ORDER BY window_key",
                )
                .unwrap();
            stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, "five_hour");
        assert_eq!(rows[0].1, Some(300));
        assert_eq!(rows[0].2, 18.0);
        assert_eq!(rows[0].4, 1_786_492_800, "observed_at is the fetch, not the reset");
        assert_eq!(rows[0].5, "live");
        assert_eq!(rows[0].6.as_deref(), Some("Team 5x"));
        // A window whose length the vendor never named stays unknown, not zero.
        assert_eq!(rows[1].0, "monthly_experiment");
        assert_eq!(rows[1].1, None);

        // Re-reading is free, and a re-read that happens anyway inserts nothing.
        assert_eq!(ingest(&mut conn, &dir, "claude"), Ok(()));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn a_pool_keyed_export_files_each_pool_as_its_own_series() {
        // Antigravity is the first Source whose pool is a genuine second axis:
        // both pools share both durations, so a key of the duration alone would
        // put two different pools' Limits on one row and lose one of them.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        write_file(
            &dir,
            &file_name("antigravity"),
            r#"{"schema":1,"source":"antigravity","fetched_at":1786492800,"plan":"Pro","windows":[
                {"key":"gemini:w300","window_minutes":300,"used_pct":58.0,"resets_at":1786547640},
                {"key":"3p:w300","window_minutes":300,"used_pct":12.0,"resets_at":1786547640}]}"#,
        );
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        assert_eq!(ingest(&mut conn, &dir, "antigravity"), Ok(()));
        let rows: Vec<(String, f64, String)> = conn
            .prepare(
                "SELECT window_key, used_pct, via FROM limit_readings \
                 WHERE source = 'antigravity' ORDER BY window_key",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows.len(), 2, "two pools sharing one duration are two rows");
        assert_eq!(rows[0], ("3p:w300".to_string(), 12.0, "live".to_string()));
        assert_eq!(rows[1].0, "gemini:w300");
    }

    #[test]
    fn an_unrecognised_schema_warns_instead_of_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        write_file(
            &dir,
            &file_name("claude"),
            r#"{"schema":99,"source":"claude","fetched_at":1,"windows":[
                {"key":"five_hour","used_pct":50.0,"resets_at":2}]}"#,
        );
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let err = ingest(&mut conn, &dir, "claude").unwrap_err();
        assert!(err.contains("unreadable limits export"), "{err}");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "an unknown shape is never guessed at");
    }

    #[test]
    fn an_export_naming_another_source_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        write_file(&dir, &file_name("claude"), &EXPORT.replace("\"claude\"", "\"codex\""));
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let err = ingest(&mut conn, &dir, "claude").unwrap_err();
        assert!(err.contains("describes codex"), "{err}");
    }

    #[test]
    fn a_written_artifact_round_trips_and_leaves_no_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let export = LimitsExport {
            schema: SCHEMA,
            source: "claude".to_string(),
            fetched_at: 1_786_492_800,
            plan: Some("Team 5x".to_string()),
            usage_resets_available: Some(1),
            windows: vec![WindowExport {
                key: "five_hour".to_string(),
                window_minutes: Some(300),
                used_pct: 18.0,
                resets_at: 1_786_503_900,
            }],
        };
        write(&dir, &export).unwrap();

        let read = super::read(&dir, "claude").unwrap();
        assert_eq!(read.source, "claude");
        assert_eq!(read.usage_resets_available, Some(1));
        assert_eq!(read.windows[0].used_pct, 18.0);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "the staging file is renamed, not left behind");
    }

    #[test]
    fn one_key_grammar_serves_every_producer() {
        assert_eq!(window_key(300), "w300");
        assert_eq!(window_key(10080), "w10080");
        assert_eq!(window_key(10081), "w10080", "upstream rounding drift is the same window");
        assert_eq!(window_key(4321), "w4321", "an unrecognised duration is kept, not guessed");
    }

    #[test]
    fn a_source_whose_companion_never_ran_is_not_in_trouble() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        assert_eq!(ingest(&mut conn, &tmp.path().join("nothing-here"), "claude"), Ok(()));
    }

    // grok_credit_window: the one mapper both Grok producers share. The log
    // ingest and the live Companion pass the identical `config` shape through it.

    #[test]
    fn a_weekly_config_becomes_one_window_through_the_shared_grammar() {
        let config: Value = serde_json::from_str(
            r#"{"creditUsagePercent":16,
                "currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY",
                    "start":"2026-07-05T00:00:00.000000+00:00",
                    "end":"2026-07-12T00:00:00.000000+00:00"}}"#,
        )
        .unwrap();
        let w = grok_credit_window(&config).unwrap();
        assert_eq!(w.key, "w10080", "the same key the log path stores, so one series");
        assert_eq!(w.window_minutes, Some(10_080));
        assert_eq!(w.used_pct, 16.0);
        assert_eq!(w.resets_at, iso_to_epoch("2026-07-12T00:00:00").unwrap());
    }

    #[test]
    fn an_absent_percent_is_zero_used_not_a_missing_window() {
        // proto3 omits zero-valued scalars, so the start of every window arrives
        // with no `creditUsagePercent` — dropping it would lose those readings.
        let config: Value = serde_json::from_str(
            r#"{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-07-12T00:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(grok_credit_window(&config).unwrap().used_pct, 0.0);
    }

    #[test]
    fn a_config_this_card_cannot_place_yields_no_window() {
        // No reset, and a period type nobody has seen: neither is placeable, and
        // an unnameable window is unknown rather than guessed.
        for config in [
            r#"{"creditUsagePercent":10,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY"}}"#,
            r#"{"creditUsagePercent":10,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_FORTNIGHTLY","end":"2026-07-12T00:00:00Z"}}"#,
        ] {
            assert!(grok_credit_window(&serde_json::from_str(config).unwrap()).is_none(), "{config}");
        }
    }

    #[test]
    fn the_deprecated_reset_mirror_is_the_fallback() {
        let config: Value = serde_json::from_str(
            r#"{"creditUsagePercent":5,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY"},
                "billingPeriodEnd":"2026-07-12T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            grok_credit_window(&config).unwrap().resets_at,
            iso_to_epoch("2026-07-12T00:00:00").unwrap(),
        );
    }
}
