// The Limits Export Artifact — the one contract between the Companions, which
// write these files, and the scan, which reads them like any other file
// (ADR-0019). Both sides use these types, so a field can never be spelled one
// way by the writer and another by the reader.
//
// One Artifact per `live` Source, named `<source>.tokenledger-limits.json`, in a
// directory the app owns. It carries Limit Readings — never tokens, never usage —
// which is the whole of what the Companion is allowed to learn.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::unchanged;
use crate::db::{self, set_file_state};
use crate::types::{FileState, LimitReading, ReadingProvenance};

/// The Model-scope grammar an export's `model_scope` is written in, re-exported
/// so a Companion writes the same one the Ledger reads.
pub use crate::types::ModelScope;

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
        // Grok's credit window is not in the estimate map; its Readings stay
        // display-only until a ticket proves what it meters.
        evidence: WindowEvidence::default(),
    })
}

/// Bump when the shape changes. An Artifact declaring a schema the reader does
/// not know is a malformed instance of a supported shape (ADR-0015): it warns
/// and is not read, rather than being guessed at.
pub const SCHEMA: u32 = 4;

fn supported_schema(schema: u32) -> bool {
    // An explicit accept-list, never a range: a range pre-accepts every future
    // bump without a decision. 4 adds `account_id`; 3 added window evidence.
    schema == 1 || schema == 2 || schema == 3 || schema == SCHEMA
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
    /// The metering regime in force, as the Companion that read the response
    /// identifies it. One meter reports every window in one fetch, so this sits
    /// on the export rather than on each of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metering_regime: Option<String>,
    /// The vendor's own stable opaque account identity, as the Companion read it
    /// beside the credential it fetched with — Codex's `tokens.account_id`,
    /// Claude's `account_uuid`. Never an email, token, or anything reversible;
    /// one fetch answers for one account, so it sits on the export. Absent means
    /// the Companion could not prove it, never "the same account as last time".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
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
    /// What this window proves about itself, where the Companion could tell.
    #[serde(default, skip_serializing_if = "WindowEvidence::is_unknown")]
    pub evidence: WindowEvidence,
}

/// The evidence facts a Companion can prove about one window from the response
/// it already read — carrying more of one fetch, never making another. Absent is
/// unknown, and unknown is never a wildcard: a window missing these is still a
/// Reading the card draws, just not one an estimate may be derived from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowEvidence {
    /// The vendor's own identity for this Limit, or an adapter-defined canonical
    /// one whose one-to-one mapping the Companion documents. A display label,
    /// a slug, or the duration alone is not one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_id: Option<String>,
    /// Model scope in the stored grammar (`ModelScope`): `all` for a window that
    /// meters the whole Source, or a sorted JSON array of raw logged Model
    /// identities. A vendor's display name is not a Model mapping, so a window
    /// scoped by one has no scope here at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_scope: Option<String>,
}

impl WindowEvidence {
    /// Nothing proven — the shape every Companion but Claude's writes today, and
    /// what every pre-schema-3 export carries.
    fn is_unknown(&self) -> bool {
        self == &WindowEvidence::default()
    }
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
            provenance: ReadingProvenance {
                limit_id: w.evidence.limit_id.clone(),
                metering_regime: export.metering_regime.clone(),
                model_scope: w.evidence.model_scope.as_deref().and_then(ModelScope::parse),
                // The Companion's own fact: it read this identity beside the
                // credential it fetched with, so every window in the export is
                // an observation of that account at `fetched_at`.
                account_id: export.account_id.clone(),
                // Coverage is not the Companion's to prove — it is a fact about
                // the scan's own history with this Source, so `ingest` computes
                // it where that history lives.
                covered_from: None,
                // One fetch reports each window once, so `observed_at` already
                // orders these Readings and no separate order is needed —
                // `observed_at` plus a source order is only *sufficient*, not
                // required, and the contract asks for an order only where two
                // Readings share an instant. Two fetches inside one second would
                // be that case, and the file-state gate on re-ingest makes it
                // near-unreachable; a consumer must still treat any pair that
                // does share an instant as unordered rather than guessing.
                source_order: None,
                external_activity: None,
            },
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

    let mut rows = readings(&export);
    let floor = coverage_floor(conn, &export).map_err(|e| e.to_string())?;
    for row in &mut rows {
        row.provenance.covered_from = floor;
    }
    db::insert_limit_readings(conn, &rows).map_err(|e| e.to_string())?;
    set_file_state(conn, &path.to_string_lossy(), state).map_err(|e| e.to_string())?;
    Ok(())
}

/// The earliest instant these Readings may claim local capture covers, or `None`
/// where nothing can be claimed at all.
///
/// Three components, combined by taking the latest. The account may only be
/// claimed forward — from the first observation that named it, never backfilled
/// onto history the app merely holds files for. Coverage is proven only past the
/// newest unreadable Artifact, whose content could reach anything up to its own
/// mtime — content is never newer than its file (ADR-0017). And no claim may
/// fall below one this Source and account already carries: `unreadable_artifacts`
/// is *current* state, rewritten clean on the next scan after the file is
/// pruned, and current state is not historical proof — a deleted unreadable is
/// indistinguishable from a repaired one, and its gap does not close. The
/// Readings themselves are the durable memory of every floor ever enforced, so
/// the highest stored claim ratchets every later one.
///
/// An unreadable whose mtime is unknown bounds nothing, so that pass proves
/// nothing: the new Readings claim nothing, and stored claims are left alone,
/// which is the documented rule for a pass that proves nothing.
///
/// Withdrawal of stored claims happens at the discovery site
/// (`db::record_unreadable`), not here — an unchanged export file skips ingest
/// entirely, and a withdrawal that waited for the vendor to answer differently
/// would not be one. The ratchet is also what keeps the per-row upsert honest:
/// a new pass computes a floor at least as high as anything stored, so its
/// newest-proof COALESCE can never quietly lower a withdrawn claim.
fn coverage_floor(conn: &Connection, export: &LimitsExport) -> rusqlite::Result<Option<i64>> {
    let Some(account_id) = &export.account_id else {
        // Coverage is a fact about a Source *and account*; with no account there
        // is nothing it could make eligible.
        return Ok(None);
    };

    // `.optional()` because a Source that never had an unreadable has no row at
    // all; the aggregate below always yields exactly one row and needs none.
    let unreadable: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT count, max_mtime FROM unreadable_artifacts WHERE source = ?1",
            [&export.source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let unreadable_floor = match unreadable {
        Some((count, Some(max_mtime))) if count > 0 => Some(max_mtime + 1),
        Some((count, None)) if count > 0 => return Ok(None),
        _ => None,
    };

    let (first_observed, highest_claim): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT MIN(observed_at), MAX(covered_from) FROM limit_readings \
         WHERE source = ?1 AND account_id = ?2",
        rusqlite::params![export.source, account_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let claimed_from = first_observed.unwrap_or(export.fetched_at);

    Ok(Some(
        [unreadable_floor, highest_claim]
            .into_iter()
            .flatten()
            .fold(claimed_from, i64::max),
    ))
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
    fn an_exports_evidence_reaches_the_readings_it_describes() {
        let export = LimitsExport {
            schema: SCHEMA,
            source: "claude".to_string(),
            fetched_at: 1_786_492_800,
            plan: Some("Max 5x".to_string()),
            metering_regime: Some("claude:usage_limits".to_string()),
            account_id: None,
            usage_resets_available: None,
            windows: vec![
                WindowExport {
                    key: "seven_day".to_string(),
                    window_minutes: Some(10080),
                    used_pct: 35.0,
                    resets_at: 1_786_503_900,
                    evidence: WindowEvidence {
                        limit_id: Some("weekly_all".to_string()),
                        model_scope: ModelScope::All.stored(),
                    },
                },
                WindowExport {
                    key: "seven_day_fable_5".to_string(),
                    window_minutes: Some(10080),
                    used_pct: 30.0,
                    resets_at: 1_786_503_900,
                    evidence: WindowEvidence::default(),
                },
            ],
        };
        let readings = readings(&export);

        // The source-wide window carries what the Companion proved.
        assert_eq!(
            readings[0].provenance,
            ReadingProvenance {
                limit_id: Some("weekly_all".to_string()),
                metering_regime: Some("claude:usage_limits".to_string()),
                model_scope: Some(ModelScope::All),
                // A fetch says what the vendor answered now, not who it answered
                // for over the stretch a Record fell in.
                account_id: None,
                covered_from: None,
                source_order: None,
                external_activity: None,
            },
        );
        // The model-scoped one keeps the regime it was metered under and proves
        // nothing else, so it stays Blocked rather than borrowing its sibling's.
        assert_eq!(readings[1].provenance.limit_id, None);
        assert_eq!(readings[1].provenance.model_scope, None);
        assert_eq!(
            readings[1].provenance.metering_regime.as_deref(),
            Some("claude:usage_limits"),
        );
    }

    #[test]
    fn a_window_that_proves_nothing_writes_no_evidence_at_all() {
        // The evidence fields are omitted rather than written empty, so a
        // Companion with nothing to prove writes the same bytes it always did
        // and an older reader finds nothing new to trip over.
        let export = LimitsExport {
            schema: SCHEMA,
            source: "grok".to_string(),
            fetched_at: 1_786_492_800,
            plan: None,
            metering_regime: None,
            account_id: None,
            usage_resets_available: None,
            windows: vec![WindowExport {
                key: "w10080".to_string(),
                window_minutes: Some(10080),
                used_pct: 4.0,
                resets_at: 1_786_503_900,
                evidence: WindowEvidence::default(),
            }],
        };
        let written = serde_json::to_string(&export).unwrap();
        assert!(!written.contains("evidence"), "{written}");
        assert!(!written.contains("metering_regime"), "{written}");
        assert_eq!(readings(&export)[0].provenance, ReadingProvenance::default());
    }

    #[test]
    fn an_export_from_before_the_evidence_fields_still_reads() {
        // Schema 2 on disk, written by a Companion that had no evidence to give.
        // It is still a Reading the card draws; it is simply not evidence.
        let raw = r#"{"schema":2,"source":"claude","fetched_at":1786492800,
            "plan":"Max 5x","windows":[{"key":"five_hour","window_minutes":300,
            "used_pct":18.0,"resets_at":1786503900}]}"#;
        let export: LimitsExport = serde_json::from_str(raw).unwrap();
        assert!(supported_schema(export.schema));
        let readings = readings(&export);
        assert_eq!(readings[0].used_pct, 18.0);
        assert_eq!(readings[0].provenance, ReadingProvenance::default());
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
            metering_regime: Some("claude:usage_limits".to_string()),
            account_id: None,
            usage_resets_available: Some(1),
            windows: vec![WindowExport {
                key: "five_hour".to_string(),
                window_minutes: Some(300),
                used_pct: 18.0,
                resets_at: 1_786_503_900,
                evidence: WindowEvidence {
                    limit_id: Some("session".to_string()),
                    model_scope: ModelScope::All.stored(),
                },
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

    /// Ingest helper: one account-bearing export whose only window resets well
    /// after the fetch, written and ingested. The file's mtime is set to the
    /// fetch instant so the file-state gate sees every pass as a new file —
    /// consecutive writes inside one test land in the same wall-clock second.
    fn ingest_at(conn: &mut Connection, dir: &Path, fetched_at: i64, account: &str) {
        let export = LimitsExport {
            schema: SCHEMA,
            source: "codex".to_string(),
            fetched_at,
            plan: Some("plus".to_string()),
            metering_regime: Some("codex:rate_limits".to_string()),
            account_id: Some(account.to_string()),
            usage_resets_available: None,
            windows: vec![WindowExport {
                key: "w10080".to_string(),
                window_minutes: Some(10_080),
                used_pct: 40.0,
                resets_at: fetched_at + 500_000,
                evidence: WindowEvidence {
                    limit_id: Some("codex:w10080".to_string()),
                    model_scope: ModelScope::All.stored(),
                },
            }],
        };
        write(dir, &export).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path_in(dir, "codex"))
            .unwrap()
            .set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(fetched_at as u64),
            )
            .unwrap();
        ingest(conn, dir, "codex").unwrap();
    }

    fn coverage_of(conn: &Connection, observed_at: i64) -> Option<i64> {
        conn.query_row(
            "SELECT covered_from FROM limit_readings WHERE observed_at = ?1",
            [observed_at],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn coverage_is_claimed_forward_from_the_first_observation_of_the_account() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        // The first pass that names the account starts the clock at itself:
        // history the app merely holds files for is never claimed.
        ingest_at(&mut conn, &dir, 1_786_492_800, "acct-a");
        assert_eq!(coverage_of(&conn, 1_786_492_800), Some(1_786_492_800));

        // A later pass claims from that same first observation, not from its
        // own fetch — the claim grows at the front, never backfills at the back.
        ingest_at(&mut conn, &dir, 1_786_496_400, "acct-a");
        assert_eq!(coverage_of(&conn, 1_786_496_400), Some(1_786_492_800));

        // A different account starts its own clock; acct-a's history is not its.
        ingest_at(&mut conn, &dir, 1_786_500_000, "acct-b");
        assert_eq!(coverage_of(&conn, 1_786_500_000), Some(1_786_500_000));
    }

    #[test]
    fn an_export_with_no_account_claims_no_coverage() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let mut export = serde_json::from_str::<LimitsExport>(EXPORT).unwrap();
        export.fetched_at = 1_786_492_800;
        write(&dir, &export).unwrap();
        ingest(&mut conn, &dir, "claude").unwrap();

        // Coverage is a fact about a Source and account; with neither proven
        // there is nothing it could make eligible.
        assert_eq!(coverage_of(&conn, 1_786_492_800), None);
    }

    #[test]
    fn an_unreadable_artifact_bounds_every_claim_after_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        // An unreadable file could hold usage as late as its own mtime, so a
        // claim may only start past it — however long the account has been
        // observed.
        ingest_at(&mut conn, &dir, 1_786_492_800, "acct-a");
        db::record_unreadable(&conn, &[unreadable("codex", 1, Some(1_786_496_000))]).unwrap();
        ingest_at(&mut conn, &dir, 1_786_500_000, "acct-a");
        assert_eq!(coverage_of(&conn, 1_786_500_000), Some(1_786_496_001));

        // One whose mtime is unknown bounds nothing: the pass proves nothing,
        // and a Reading that proves nothing claims nothing.
        db::record_unreadable(&conn, &[unreadable("codex", 1, None)]).unwrap();
        ingest_at(&mut conn, &dir, 1_786_503_600, "acct-a");
        assert_eq!(coverage_of(&conn, 1_786_503_600), None);
    }

    #[test]
    fn a_discovery_withdraws_stored_claims_without_waiting_for_ingest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        // Two stored claims, then the scan finds an unreadable Artifact newer
        // than one of them. The export file has not changed — no ingest runs —
        // and the affected claim must still be withdrawn (spec: corrections
        // take effect on the next evaluation).
        ingest_at(&mut conn, &dir, 1_786_492_800, "acct-a");
        ingest_at(&mut conn, &dir, 1_786_496_400, "acct-a");
        db::record_unreadable(&conn, &[unreadable("codex", 1, Some(1_786_495_000))]).unwrap();

        for observed_at in [1_786_492_800, 1_786_496_400] {
            assert_eq!(coverage_of(&conn, observed_at), Some(1_786_495_001));
        }

        // Idempotent, and an older discovery never lowers a raised claim.
        db::record_unreadable(&conn, &[unreadable("codex", 1, Some(1_786_400_000))]).unwrap();
        assert_eq!(coverage_of(&conn, 1_786_492_800), Some(1_786_495_001));
    }

    #[test]
    fn a_pruned_unreadable_does_not_close_the_gap_it_proved() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("limits");
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        // Discovery raises the floor; then the unreadable file is pruned and the
        // next scan rewrites the source's row CLEAN — current state, which is
        // not historical proof. The usage that file held is still missing, so a
        // later pass must not claim back across the gap: the highest stored
        // claim is the durable memory the current-state table is not.
        ingest_at(&mut conn, &dir, 1_786_492_800, "acct-a");
        db::record_unreadable(&conn, &[unreadable("codex", 1, Some(1_786_495_000))]).unwrap();
        db::record_unreadable(&conn, &[unreadable("codex", 0, None)]).unwrap();

        ingest_at(&mut conn, &dir, 1_786_500_000, "acct-a");
        assert_eq!(coverage_of(&conn, 1_786_500_000), Some(1_786_495_001));
    }

    fn unreadable(
        source: &str,
        count: u64,
        max_mtime: Option<i64>,
    ) -> crate::types::SourceStatus {
        crate::types::SourceStatus {
            source: source.to_string(),
            events_inserted: 0,
            lines_skipped: 0,
            artifacts_unreadable: count,
            unreadable_max_mtime: max_mtime,
            error: None,
        }
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
