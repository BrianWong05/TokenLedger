// The Claude desktop app's own usage history — a third channel for a Claude
// Limit Reading, beside the Companion's live fetch (ADR-0019) and the
// statusline tap (TOKL-34, ADR-0027).
//
// The Claude desktop app keeps its own record of the vendor's utilization
// percentages in its Electron userData directory — `plan-usage-history.json`,
// rewritten whole (atomic rename) at most every 270 seconds while the app runs,
// one entry roughly every fifteen minutes, thirty days deep. Nobody wrote it for
// us: it is an already-populated third-party cache, which is precisely the kind
// of Artifact ADR-0013 allows the scan to read. Nothing here signs in, presents
// a credential, or asks a vendor anything; the file is read exactly as the scan
// reads a transcript, and a machine without the desktop app simply has no file.
//
// Two things come out of one pass, and they are not the same kind of fact.
//
// A Limit READING needs a reset instant: `resets_at` is NOT NULL and part of
// the Ledger's primary key, because an observation that cannot say which epoch
// it belongs to cannot be compared with another. This file names no reset at
// all. So an entry is stored as a Reading only where the Ledger ALREADY holds
// an epoch it falls inside — proven by another producer, not invented here —
// and an entry with no such epoch is not stored at all.
//
// What those unplaceable entries leave is CURRENT STATE — the same status as
// Codex's Usage Reset count (glossary: Usage Reset): what the Source says right
// now, with no claim to be history. It rides the card from a Limit State
// Artifact this module rename-writes beside the Companion's export, and the
// Limits query overlays it per window. That is the whole answer to "the page
// draws an expired epoch as unused while the desktop app plainly knows better".
//
// Readings from here carry no account identity. The Companion's `account_id` is
// the vendor's `account_uuid`; this file names an ORG uuid, a different identity
// for a different thing, and stamping one as the other would file two Sources'
// history into one Series. Unproven identity is not a wildcard, so these
// Readings are display evidence and nothing more — `SeriesKey::of` refuses them
// on its ordinary rule, with no special case in either direction.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use crate::adapters::unchanged;
use crate::db::{self, set_file_state};
use crate::limits_artifact::{
    self, LimitState, LimitStateWindow, CLAUDE_METERING_REGIME, STATE_SCHEMA,
};
use crate::types::{FileState, LimitReading, ReadingProvenance};

/// The Source whose desktop app writes this history.
pub const SOURCE: &str = "claude";

/// The channel stamped on every Reading and state figure from this file, in the
/// `LimitReading::via` vocabulary.
pub const VIA: &str = "desktop";

/// The parser's own version, carried in the file-state row's `byte_offset` slot
/// the way every other adapter carries it: bumping it re-reads a file whose size
/// and mtime never changed, which is the only way a corrected mapping reaches
/// history the old mapping already walked.
const PARSER_VERSION: i64 = 1;

/// The vendor's own array key. Spelt once, here: these are the desktop app's
/// ENTRIES, and a Limit Reading is never called a sample (CONTEXT.md).
const ENTRIES_KEY: &str = "samples";

/// The desktop app's short code for each window → the vendor's own window key
/// and the window's length in minutes, taken from the desktop app's own code.
/// The vendor keys are deliberately the SAME ones the Companion and the
/// statusline tap write, so a figure from this channel joins the timeline those
/// producers already keep rather than opening a second bar for one window.
///
/// `omelette_promotional` has no published length, so it stays unknown rather
/// than borrowing a sibling's — the card then draws a bar with no time tick.
/// Two codes are deliberately absent: `xu` is extra-usage SPEND, which is not a
/// rolling-window Limit at all, and any code nobody has mapped yet is dropped
/// rather than filed under a key invented for it.
const WINDOWS: [(&str, &str, Option<i64>); 8] = [
    ("fh", "five_hour", Some(300)),
    ("sd", "seven_day", Some(10_080)),
    ("so", "seven_day_opus", Some(10_080)),
    ("sn", "seven_day_sonnet", Some(10_080)),
    ("oa", "seven_day_oauth_apps", Some(10_080)),
    ("cw", "seven_day_cowork", Some(10_080)),
    ("om", "seven_day_omelette", Some(10_080)),
    ("op", "omelette_promotional", None),
];

/// How far back an entry may reach for an epoch when its window's length is
/// unknown. Every named window the vendor meters weekly or shorter, so the
/// weekly span is the widest an unknown one could plausibly be — and a bound
/// too wide only ever refuses to place an entry the epoch does not cover,
/// because the epoch chosen is the closest one after it.
const UNKNOWN_WINDOW_MINUTES: i64 = 10_080;

/// One entry of the desktop app's history: when the app recorded it, the org it
/// recorded it for, and the figures it recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Epoch SECONDS. The file states milliseconds; the conversion happens once,
    /// at the parse boundary, and nothing downstream sees the vendor's unit.
    pub observed_at: i64,
    /// The organization the desktop app was signed into. Absent in shape 1, and
    /// absent where shape 2 writes a null.
    pub org: Option<String>,
    pub figures: Vec<Figure>,
}

/// One window's figure inside one entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub window_key: &'static str,
    pub window_minutes: Option<i64>,
    pub used_pct: f64,
}

/// Parse the desktop app's history, accepting both shapes it has written:
/// shape 2 nests the figures under `u` and names an org, shape 1 carries `fh`
/// and `sd` beside the timestamp and names none.
///
/// `None` is "this file tells us nothing" — unreadable, truncated mid-rewrite,
/// or a shape nobody has mapped. We do not own this file, so a shape we cannot
/// read is an ABSENCE rather than a fault: an absence is retried on the next
/// pass, and warning about a third party's private format would report the
/// vendor's release cadence as the app's trouble (contrast
/// `limits_artifact::ingest`, which owns its shape and so must warn about it).
pub fn parse_history(raw: &str) -> Option<Vec<Entry>> {
    let document: Value = serde_json::from_str(raw).ok()?;
    let version = document.get("version")?.as_u64()?;
    if version != 1 && version != 2 {
        return None;
    }
    let entries = document.get(ENTRIES_KEY)?.as_array()?;
    Some(entries.iter().filter_map(|entry| one(entry, version)).collect())
}

/// One entry, or `None` where this one entry is unreadable — the file is
/// rewritten whole, so an entry that does not parse is a shape question about
/// that entry alone and never a reason to drop the thirty days around it.
fn one(entry: &Value, version: u64) -> Option<Entry> {
    // Milliseconds in the file, seconds everywhere in the Ledger. A raw
    // millisecond value would sort fifty thousand years into the future and
    // stand as the newest epoch forever, so the unit is converted here and the
    // conversion exists in exactly one place.
    let millis = entry.get("t").and_then(|t| t.as_i64().or_else(|| t.as_f64().map(|t| t as i64)))?;
    let figures = if version == 2 { entry.get("u")? } else { entry };
    let figures = figures.as_object()?;
    Some(Entry {
        observed_at: millis / 1_000,
        org: entry.get("org").and_then(Value::as_str).map(str::to_string),
        // Driven by the map rather than by the file, so an unmapped code cannot
        // reach a window key by accident.
        figures: WINDOWS
            .iter()
            .filter_map(|&(code, window_key, window_minutes)| {
                let used_pct = figures.get(code).and_then(Value::as_f64)?;
                Some(Figure { window_key, window_minutes, used_pct })
            })
            .collect(),
    })
}

/// The entries of the organization in force, which is the one the NEWEST entry
/// names. A person who switched orgs leaves the previous org's history in the
/// same file, and two orgs' utilization of one window are two different
/// quantities — so entries naming a different org are dropped rather than
/// interleaved. Entries naming NO org are kept: shape 1 never named one, and an
/// unnamed org is unknown, which is not evidence of a different one.
fn of_the_current_org(entries: Vec<Entry>) -> Vec<Entry> {
    let current = entries
        .iter()
        .max_by_key(|entry| entry.observed_at)
        .and_then(|entry| entry.org.clone());
    let Some(current) = current else {
        return entries;
    };
    entries
        .into_iter()
        .filter(|entry| match &entry.org {
            Some(org) => *org == current,
            None => true,
        })
        .collect()
}

/// Every reset instant the Ledger already holds a Reading for, in one Limit,
/// ascending. Exported so a profile can `EXPLAIN` the statement this module
/// actually issues rather than a copy of it.
pub const KNOWN_EPOCHS_SQL: &str = "SELECT DISTINCT resets_at FROM limit_readings \
     WHERE source = ?1 AND window_key = ?2 ORDER BY resets_at";

fn known_epochs(conn: &Connection, window_key: &str) -> rusqlite::Result<Vec<i64>> {
    conn.prepare(KNOWN_EPOCHS_SQL)?
        .query_map((SOURCE, window_key), |row| row.get(0))?
        .collect()
}

/// The epoch an entry falls inside, or `None` where the Ledger knows none.
///
/// An epoch covers `(resets_at - length, resets_at)`: an entry at or after the
/// reset belongs to the NEXT epoch, and one further back than the window is
/// long belongs to an earlier one nobody has proven. Where two stored epochs
/// both qualify — which the reset stamp's own jitter produces within what is
/// plainly one window (#104) — the LARGEST is taken, so the new Reading joins
/// the same jitter band `DISPLAYED_WINDOWS_SQL` draws the card from rather than
/// landing just below it and being filtered out of the very card it belongs to.
fn epoch_for(epochs: &[i64], observed_at: i64, window_minutes: Option<i64>) -> Option<i64> {
    let span = window_minutes.unwrap_or(UNKNOWN_WINDOW_MINUTES) * 60;
    epochs
        .iter()
        .rev()
        .find(|&&resets_at| resets_at > observed_at && resets_at - observed_at <= span)
        .copied()
}

/// One entry's figure as a Limit Reading of a known epoch.
///
/// `plan` and `account_id` stay unknown: the desktop app's history states
/// neither, and an unknown identity is never a wildcard. That is also what keeps
/// this row harmless where it lands on a key a Companion Reading already holds
/// — `insert_limit_readings` COALESCEs an unknown over a stored fact and
/// contradicts nothing, so a proven row keeps its identity.
fn reading(entry: &Entry, figure: &Figure, resets_at: i64) -> LimitReading {
    LimitReading {
        source: SOURCE.to_string(),
        window_key: figure.window_key.to_string(),
        window_minutes: figure.window_minutes,
        used_pct: figure.used_pct,
        resets_at,
        observed_at: entry.observed_at,
        via: VIA.to_string(),
        plan: None,
        provenance: ReadingProvenance {
            // The one fact this file does prove: these percentages are Claude's
            // usage limits, the same meter every Claude producer reports.
            metering_regime: Some(CLAUDE_METERING_REGIME.to_string()),
            ..ReadingProvenance::default()
        },
    }
}

/// Read the desktop app's history and append what it proves, returning how many
/// Limit Readings were written or genuinely revised — the change signal the scan
/// reports and an open Limits page reissues its query on.
///
/// Absent file → nothing to do, and not an error. Unreadable file → also nothing
/// to do, and no file state recorded, so the next pass reads it again: the app
/// rewrites this file whole, and a read that caught it mid-rename must not be
/// remembered as the last word on it.
///
/// Idempotent twice over: the file's own state gates the re-read, and the
/// Ledger's primary key lands a re-read entry on the row already stored.
pub fn ingest(conn: &mut Connection, file: &Path, limit_exports: &Path) -> Result<u64, String> {
    let Ok(raw) = std::fs::read_to_string(file) else {
        return Ok(0);
    };
    let Ok(meta) = std::fs::metadata(file) else {
        return Ok(0);
    };
    // Exact equality on size and mtime, which is all `unchanged` offers and all
    // this file needs: the desktop app rename-writes it at most every 270
    // seconds, far wider than the one-second mtime granularity a same-second
    // rewrite would hide behind.
    let state = FileState {
        size: meta.len() as i64,
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        byte_offset: PARSER_VERSION,
    };
    if unchanged(conn, file, &state) {
        return Ok(0);
    }

    let Some(entries) = parse_history(&raw) else {
        return Ok(0);
    };
    let entries = of_the_current_org(entries);

    // Every epoch the Ledger knows, read once per window BEFORE anything is
    // written: an entry is placed against epochs another producer proved, never
    // against one an earlier entry of this same pass just created.
    let mut epochs: BTreeMap<&'static str, Vec<i64>> = BTreeMap::new();
    for figure in entries.iter().flat_map(|entry| &entry.figures) {
        if !epochs.contains_key(figure.window_key) {
            let known = known_epochs(conn, figure.window_key).map_err(|e| e.to_string())?;
            epochs.insert(figure.window_key, known);
        }
    }

    let mut rows = Vec::new();
    for entry in &entries {
        for figure in &entry.figures {
            let known = epochs.get(figure.window_key).map_or(&[][..], Vec::as_slice);
            if let Some(resets_at) = epoch_for(known, entry.observed_at, figure.window_minutes) {
                rows.push(reading(entry, figure, resets_at));
            }
        }
    }
    let written = db::insert_limit_readings(conn, &rows).map_err(|e| e.to_string())?;

    // What the newest entry says, placed where it can be and stated as unknown
    // where it cannot — current state, not history. Skipped where no Companion
    // has been given a place to write ("" is the "not configured" spelling
    // `scan::merge_limit_exports` guards with).
    if !limit_exports.as_os_str().is_empty() {
        if let Some(newest) = entries.iter().max_by_key(|entry| entry.observed_at) {
            let state = LimitState {
                schema: STATE_SCHEMA,
                source: SOURCE.to_string(),
                via: VIA.to_string(),
                observed_at: newest.observed_at,
                windows: newest
                    .figures
                    .iter()
                    .map(|figure| LimitStateWindow {
                        key: figure.window_key.to_string(),
                        window_minutes: figure.window_minutes,
                        used_pct: figure.used_pct,
                        resets_at: epoch_for(
                            epochs.get(figure.window_key).map_or(&[][..], Vec::as_slice),
                            newest.observed_at,
                            figure.window_minutes,
                        ),
                    })
                    .collect(),
            };
            limits_artifact::write_state(limit_exports, &state).map_err(|e| e.to_string())?;
        }
    }

    set_file_state(conn, &file.to_string_lossy(), state).map_err(|e| e.to_string())?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{get_file_state, open_db};
    use crate::types::ModelScope;

    /// Every code the desktop app writes, in one entry, plus one nobody has
    /// mapped — the production mix a hand-kept list is most likely to get wrong.
    const EVERY_CODE: &str = r#"{"version":2,"samples":[
        {"t":1789000000000,"org":"org-a","u":{"fh":11,"sd":22,"so":33,"sn":44,
          "oa":55,"cw":66,"om":77,"op":88,"xu":99,"zz":12}}]}"#;

    fn plant(dir: &Path, body: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("plan-usage-history.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Shift a file's mtime so a second pass in the same wall-clock second is
    /// still seen as a changed file.
    fn touch(path: &Path, mtime: i64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64))
            .unwrap();
    }

    /// A Companion's own Reading of one epoch — the shape that proves an epoch
    /// exists for a desktop entry to be placed in.
    fn live_reading(window_key: &str, used_pct: f64, observed_at: i64, resets_at: i64) -> LimitReading {
        LimitReading {
            source: SOURCE.to_string(),
            window_key: window_key.to_string(),
            window_minutes: Some(300),
            used_pct,
            resets_at,
            observed_at,
            via: "live".to_string(),
            plan: Some("Max 20x".to_string()),
            provenance: ReadingProvenance {
                account_id: Some("acct-uuid".to_string()),
                metering_regime: Some(CLAUDE_METERING_REGIME.to_string()),
                limit_id: Some("session".to_string()),
                model_scope: Some(ModelScope::All),
                covered_from: Some(0),
                ..ReadingProvenance::default()
            },
        }
    }

    fn stored(conn: &Connection) -> Vec<(String, Option<i64>, f64, i64, i64, String)> {
        conn.prepare(
            "SELECT window_key, window_minutes, used_pct, resets_at, observed_at, via \
             FROM limit_readings WHERE via = 'desktop' ORDER BY window_key, observed_at",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }

    #[test]
    fn every_mapped_code_becomes_its_vendor_window_and_nothing_else_does() {
        let entries = parse_history(EVERY_CODE).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0]
                .figures
                .iter()
                .map(|f| (f.window_key, f.window_minutes, f.used_pct))
                .collect::<Vec<_>>(),
            [
                ("five_hour", Some(300), 11.0),
                ("seven_day", Some(10_080), 22.0),
                ("seven_day_opus", Some(10_080), 33.0),
                ("seven_day_sonnet", Some(10_080), 44.0),
                ("seven_day_oauth_apps", Some(10_080), 55.0),
                ("seven_day_cowork", Some(10_080), 66.0),
                ("seven_day_omelette", Some(10_080), 77.0),
                // The one window whose length nobody has published stays
                // unknown rather than borrowing its siblings'.
                ("omelette_promotional", None, 88.0),
            ],
        );
        // Extra-usage spend is not a rolling-window Limit, and an unmapped code
        // is not a window at all. Asserted by the figures they would carry, so
        // renaming a key cannot make this pass vacuously.
        assert!(!entries[0].figures.iter().any(|f| f.used_pct == 99.0), "xu is not a window");
        assert!(!entries[0].figures.iter().any(|f| f.used_pct == 12.0), "an unknown code is dropped");
    }

    #[test]
    fn the_first_shape_still_reads_and_names_no_org() {
        let entries = parse_history(
            r#"{"version":1,"samples":[{"t":1789000000000,"fh":9,"sd":41},
                                       {"t":1789000900000,"fh":null,"sd":42}]}"#,
        )
        .unwrap();
        assert_eq!(
            entries[0].figures.iter().map(|f| (f.window_key, f.used_pct)).collect::<Vec<_>>(),
            [("five_hour", 9.0), ("seven_day", 41.0)],
        );
        assert_eq!(entries[0].org, None, "shape 1 names no org, and unknown is not a wildcard");
        // A null figure is a window the app had nothing to say about, not a zero.
        assert_eq!(
            entries[1].figures.iter().map(|f| (f.window_key, f.used_pct)).collect::<Vec<_>>(),
            [("seven_day", 42.0)],
        );
    }

    #[test]
    fn the_vendors_milliseconds_become_the_ledgers_seconds_once() {
        let entries = parse_history(r#"{"version":2,"samples":[{"t":1789000000000,"u":{"fh":5}}]}"#)
            .unwrap();
        assert_eq!(entries[0].observed_at, 1_789_000_000);
    }

    #[test]
    fn the_newest_entrys_org_is_the_one_in_force() {
        let entries = parse_history(
            r#"{"version":2,"samples":[
                {"t":1789000000000,"org":"org-old","u":{"fh":80}},
                {"t":1789000900000,"u":{"fh":10}},
                {"t":1789001800000,"org":"org-new","u":{"fh":20}}]}"#,
        )
        .unwrap();
        let kept = of_the_current_org(entries);
        assert_eq!(
            kept.iter().map(|e| (e.observed_at, e.figures[0].used_pct)).collect::<Vec<_>>(),
            // The other org's entry is dropped; the one naming no org is kept.
            [(1_789_000_900, 10.0), (1_789_001_800, 20.0)],
        );
    }

    #[test]
    fn a_file_this_side_cannot_read_is_an_absence_that_is_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");

        for body in [
            "",
            "not json at all",
            // Truncated mid-rewrite: valid JSON prefix, no closing bracket.
            r#"{"version":2,"samples":[{"t":1789000000000,"u":{"fh":5}}"#,
            // A shape nobody has mapped is not guessed at.
            r#"{"version":9,"samples":[{"t":1789000000000,"u":{"fh":5}}]}"#,
            r#"{"samples":[]}"#,
        ] {
            assert_eq!(parse_history(body), None, "{body}");
            let path = plant(&tmp.path().join("desktop"), body);
            assert_eq!(ingest(&mut conn, &path, &exports), Ok(0), "{body}");
            // No file state, so the next pass reads it again — the app rewrites
            // this file whole, and a read that caught a rename is not the last
            // word on it.
            assert!(
                get_file_state(&conn, &path.to_string_lossy()).unwrap().is_none(),
                "{body}",
            );
            assert!(!limits_artifact::state_path_in(&exports, SOURCE).exists(), "{body}");
        }
    }

    #[test]
    fn an_entry_is_stored_only_inside_an_epoch_the_ledger_already_knows() {
        // One five-hour epoch, proven by a Companion Reading.
        let reset = 1_789_018_000;
        let epochs = [reset];
        let hour = 3_600;

        // Inside (reset - 5h, reset).
        assert_eq!(epoch_for(&epochs, reset - hour, Some(300)), Some(reset));
        assert_eq!(epoch_for(&epochs, reset - 1, Some(300)), Some(reset));
        // At the reset, and after it: the next epoch's, which nobody proved.
        assert_eq!(epoch_for(&epochs, reset, Some(300)), None);
        assert_eq!(epoch_for(&epochs, reset + 1, Some(300)), None);
        // A full window before the reset is the epoch's own start, and inside
        // it; anything earlier belongs to an epoch nobody has proven.
        assert_eq!(epoch_for(&epochs, reset - 5 * hour, Some(300)), Some(reset));
        assert_eq!(epoch_for(&epochs, reset - 5 * hour - 1, Some(300)), None);
        // A window nobody has published a length for is bounded weekly.
        assert_eq!(epoch_for(&epochs, reset - 5 * hour, None), Some(reset));
        assert_eq!(epoch_for(&epochs, reset - 10_080 * 60 - 1, None), None);
        // Two qualifying epochs: the larger, so the row joins the band the card
        // is drawn from rather than sitting just under it.
        let jittered = [reset - 90, reset];
        assert_eq!(epoch_for(&jittered, reset - hour, Some(300)), Some(reset));
        // A Limit the Ledger holds nothing for places nothing.
        assert_eq!(epoch_for(&[], reset - hour, Some(300)), None);
    }

    #[test]
    fn an_ordinary_pass_stores_the_placeable_entries_and_states_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let reset = 1_789_018_000;

        // The Ledger knows one five-hour epoch and nothing weekly at all.
        db::insert_limit_readings(&mut conn, &[live_reading("five_hour", 30.0, reset - 7_200, reset)])
            .unwrap();

        let path = plant(
            &tmp.path().join("desktop"),
            &format!(
                r#"{{"version":2,"samples":[
                    {{"t":{}000,"org":"org-a","u":{{"fh":40,"sd":12}}}},
                    {{"t":{}000,"org":"org-a","u":{{"fh":45,"sd":13}}}}]}}"#,
                reset - 3_600,
                reset - 900,
            ),
        );
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(2), "two five-hour figures placed");

        let rows = stored(&conn);
        assert_eq!(
            rows,
            [
                ("five_hour".to_string(), Some(300), 40.0, reset, reset - 3_600, "desktop".to_string()),
                ("five_hour".to_string(), Some(300), 45.0, reset, reset - 900, "desktop".to_string()),
            ],
            "the weekly figures have no known epoch and are not stored",
        );

        // The state Artifact states both: the placed window with its epoch, the
        // unplaceable one with none.
        let state = limits_artifact::read_state(&exports, SOURCE).unwrap();
        assert_eq!(state.via, "desktop");
        assert_eq!(state.observed_at, reset - 900, "the newest accepted entry");
        assert_eq!(
            state.windows,
            [
                LimitStateWindow {
                    key: "five_hour".to_string(),
                    window_minutes: Some(300),
                    used_pct: 45.0,
                    resets_at: Some(reset),
                },
                LimitStateWindow {
                    key: "seven_day".to_string(),
                    window_minutes: Some(10_080),
                    used_pct: 13.0,
                    // Unknown, never a zero and never a guessed instant.
                    resets_at: None,
                },
            ],
        );
    }

    #[test]
    fn the_state_artifact_is_not_an_export_and_is_never_ingested_as_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let path = plant(
            &tmp.path().join("desktop"),
            r#"{"version":2,"samples":[{"t":1789000000000,"org":"org-a","u":{"fh":7}}]}"#,
        );
        ingest(&mut conn, &path, &exports).unwrap();

        let written = limits_artifact::state_path_in(&exports, SOURCE);
        assert!(written.to_string_lossy().ends_with(limits_artifact::STATE_SUFFIX));
        // The export reader is addressed by name and finds nothing; the export
        // NAME grammar does not claim this file either. Were the suffixes to
        // converge, the scan would file current state as Reading history.
        assert_eq!(limits_artifact::read(&exports, SOURCE).map(|e| e.source), None);
        assert_eq!(limits_artifact::source_key(&written), None);
        // And the rename-write leaves no staging file behind.
        let leftovers: Vec<_> = std::fs::read_dir(&exports)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn no_configured_export_directory_means_no_state_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let path = plant(
            &tmp.path().join("desktop"),
            r#"{"version":2,"samples":[{"t":1789000000000,"org":"org-a","u":{"fh":7}}]}"#,
        );
        // "" is the not-configured spelling; a relative lookup through the
        // process CWD is exactly what it exists to prevent.
        assert_eq!(ingest(&mut conn, &path, Path::new("")), Ok(0));
        assert!(!Path::new("claude.tokenledger-limit-state.json").exists());
    }

    #[test]
    fn a_second_pass_over_an_unchanged_file_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let reset = 1_789_018_000;
        db::insert_limit_readings(&mut conn, &[live_reading("five_hour", 30.0, reset - 7_200, reset)])
            .unwrap();

        let dir = tmp.path().join("desktop");
        let path = plant(
            &dir,
            &format!(r#"{{"version":2,"samples":[{{"t":{}000,"u":{{"fh":40}}}}]}}"#, reset - 3_600),
        );
        touch(&path, reset - 3_500);
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(1));
        let recorded = get_file_state(&conn, &path.to_string_lossy()).unwrap().unwrap();
        assert_eq!((recorded.mtime, recorded.byte_offset), (reset - 3_500, PARSER_VERSION));

        // The gate is what stops the second pass, not the Ledger's own
        // deduplication: removing the state Artifact and rescanning proves the
        // file was never re-read at all, where a count of zero alone would have
        // been the primary key's doing either way.
        std::fs::remove_file(limits_artifact::state_path_in(&exports, SOURCE)).unwrap();
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(0), "the file-state gate holds");
        assert!(
            limits_artifact::read_state(&exports, SOURCE).is_none(),
            "an unchanged file is not parsed, so nothing is restated",
        );

        // A changed file is read again, and only its new entry counts.
        plant(
            &dir,
            &format!(
                r#"{{"version":2,"samples":[{{"t":{}000,"u":{{"fh":40}}}},
                                            {{"t":{}000,"u":{{"fh":44}}}}]}}"#,
                reset - 3_600,
                reset - 900,
            ),
        );
        touch(&path, reset - 800);
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(1), "only the entry nobody had");
        assert_eq!(stored(&conn).len(), 2);
    }

    #[test]
    fn a_desktop_row_on_a_companion_rows_key_leaves_its_identity_alone() {
        // The exact five-column collision: same source, window, epoch, instant
        // and percentage. `via` is the first writer's stand and is never
        // revised, and an unknown identity must COALESCE over a proven one
        // rather than contradicting it — a contradiction is dropped whole, and
        // a blend would file a Companion's Series under nobody's account.
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let reset = 1_789_018_000;
        let at = reset - 3_600;
        db::insert_limit_readings(&mut conn, &[live_reading("five_hour", 40.0, at, reset)]).unwrap();

        // The file names an ORG, which is a different identity for a different
        // thing than the Companion's account. The second entry collides with
        // nothing, so what it stores is visible on its own row.
        let path = plant(
            &tmp.path().join("desktop"),
            &format!(
                r#"{{"version":2,"samples":[
                    {{"t":{at}000,"org":"org-uuid","u":{{"fh":40}}}},
                    {{"t":{}000,"org":"org-uuid","u":{{"fh":46}}}}]}}"#,
                reset - 900,
            ),
        );
        ingest(&mut conn, &path, &exports).unwrap();

        let row = |observed_at: i64| -> (String, Option<String>, Option<String>, Option<String>, Option<String>) {
            conn.query_row(
                "SELECT via, account_id, limit_id, model_scope, plan FROM limit_readings \
                 WHERE source = 'claude' AND window_key = 'five_hour' AND observed_at = ?1",
                [observed_at],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap()
        };
        assert_eq!(
            row(at),
            (
                "live".to_string(),
                Some("acct-uuid".to_string()),
                Some("session".to_string()),
                Some("all".to_string()),
                Some("Max 20x".to_string()),
            ),
        );
        // And the row of its own carries no account at all: the org is not the
        // vendor's account identity, and filing it as one would blend two
        // different identities into one Series.
        assert_eq!(
            row(reset - 900),
            ("desktop".to_string(), None, None, None, None),
        );
    }

    #[test]
    fn desktop_rows_between_two_proven_readings_neither_break_nor_invent_an_interval() {
        // The interleaving this channel creates, end to end: two Companion
        // Readings bracketing one movement, with the desktop app's own entries
        // of the same epoch landing between them. A desktop Reading proves no
        // account, so it can never anchor and never admit evidence — but it
        // must also not END the run it sits inside, or Claude would lose every
        // Limit Evidence Interval the moment the desktop app was installed.
        use crate::types::{CtxTokens, UsageEvent};

        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let reset = 1_789_018_000;
        let (first, last) = (reset - 3_600, reset - 1_800);

        db::insert_limit_readings(
            &mut conn,
            &[
                live_reading("five_hour", 40.0, first, reset),
                live_reading("five_hour", 50.0, last, reset),
            ],
        )
        .unwrap();
        db::insert_events(
            &mut conn,
            &[UsageEvent {
                dedup_key: "desktop-interval".to_string(),
                source: SOURCE.to_string(),
                timestamp: first + 600,
                model: Some("claude-opus-4-8".to_string()),
                project: None,
                api_calls: 1,
                input_tokens: 1_200,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_5m_tokens: 0,
                cache_write_1h_tokens: 0,
                source_file: "desktop-interval.jsonl".to_string(),
                session_id: None,
                reasoning_tokens: None,
                ctx: CtxTokens::default(),
            }],
        )
        .unwrap();
        conn.execute("UPDATE events SET account_id = 'acct-uuid'", []).unwrap();

        // Two desktop entries inside the movement, rising with it.
        let path = plant(
            &tmp.path().join("desktop"),
            &format!(
                r#"{{"version":2,"samples":[
                    {{"t":{}000,"org":"org-a","u":{{"fh":43}}}},
                    {{"t":{}000,"org":"org-a","u":{{"fh":47}}}}]}}"#,
                first + 600,
                first + 1_200,
            ),
        );
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(2));

        let readings = crate::limits_evidence::stored_readings(&conn, 0).unwrap();
        assert_eq!(readings.len(), 4, "the interleaving has to be real for this to bite");
        let usage = crate::limits_evidence::matching_usage(&conn, &readings).unwrap();
        let evidence = crate::limits_evidence::derive(&readings, &usage).unwrap();

        let intervals: Vec<_> =
            evidence.partitions.iter().flat_map(|p| p.intervals.clone()).collect();
        assert_eq!(intervals.len(), 1, "one interval, between the two proven Readings");
        assert_eq!((intervals[0].from_pct, intervals[0].to_pct), (40, 50));
        assert_eq!((intervals[0].t0, intervals[0].t1), (first, last));
        assert_eq!(intervals[0].tokens, 1_200);
        // And the desktop rows bound nothing of their own: an unproven identity
        // is not a wildcard, so they are counted as a missing account rather
        // than quietly forming a Series nobody proved.
        assert_eq!(
            evidence.refusals(SOURCE, "five_hour").get(&crate::limits_evidence::ReasonCode::MissingAccountIdentity),
            Some(&2),
        );
    }

    #[test]
    fn a_newer_desktop_row_does_not_blank_the_plan_pill() {
        // Desktop Readings carry no plan, and the pill is "the plan as of the
        // newest observation NAMING one" — a newer silent Reading must not read
        // as a Source that has stopped having a plan.
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let exports = tmp.path().join("limits");
        let reset = 1_789_018_000;
        db::insert_limit_readings(&mut conn, &[live_reading("five_hour", 30.0, reset - 7_200, reset)])
            .unwrap();

        let path = plant(
            &tmp.path().join("desktop"),
            &format!(r#"{{"version":2,"samples":[{{"t":{}000,"u":{{"fh":44}}}}]}}"#, reset - 60),
        );
        assert_eq!(ingest(&mut conn, &path, &exports), Ok(1));

        let cards = crate::queries::limits(&conn, reset - 30, &exports).unwrap();
        assert_eq!(cards[0].plan.as_deref(), Some("Max 20x"));
        assert_eq!(cards[0].windows[0].used_pct, 44.0, "the newest figure is the desktop one");
    }
}
