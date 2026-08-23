//! The statusline tap as a separate process — the same boundary Claude Code's
//! statusLine spawn crosses — with its config dir and limits dir injected by
//! env, the written Export Artifact carried through the SAME schema-checked
//! ingest the app uses, and the pipe-through proven byte-identical. No
//! network anywhere: the tap's whole point is that it never fetches.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tokenledger_lib::limits_artifact;

const BIN: &str = env!("CARGO_BIN_EXE_claude-statusline-tap");

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// A status document the way Claude Code writes one: named buckets with
/// figures, a null experiment lane riding along, and unrelated fields the tap
/// must ignore.
fn payload(session_pct: f64, weekly_pct: f64, resets_at: i64) -> String {
    format!(
        r#"{{"session_id":"t","model":{{"display_name":"Fable 5"}},
            "rate_limits":{{
                "five_hour":{{"used_percentage":{session_pct},"resets_at":{resets_at}}},
                "seven_day":{{"used_percentage":{weekly_pct},"resets_at":{resets_at}}},
                "seven_day_opus":null}}}}"#,
    )
}

/// Run the tap with `cat` downstream, so the render leg is the real spawn
/// path and stdout proves the pipe-through.
fn run(config: &Path, limits: &Path, stdin: &str) -> Output {
    let mut child = Command::new(BIN)
        .arg("/bin/cat")
        .env("CLAUDE_CONFIG_DIR", config)
        .env("TOKENLEDGER_LIMITS_DIR", limits)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the tap binary must run");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn dirs_in(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let config = tmp.join("config");
    let limits = tmp.join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();
    (config, limits)
}

#[test]
fn a_snapshot_lands_through_the_real_ingest_and_the_render_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    // The identity fields the Companion also reports, from the same document.
    std::fs::write(
        config.join(".claude.json"),
        r#"{"oauthAccount":{"userRateLimitTier":"default_claude_max_5x",
            "accountUuid":"ca9db76e-0000-4000-a000-a91c5789ed3a"}}"#,
    )
    .unwrap();

    let resets = now() + 3 * 3600;
    let doc = payload(16.0, 71.0, resets);
    let output = run(&config, &limits, &doc);
    assert!(output.status.success());
    // The downstream saw exactly the bytes Claude Code sent — the tap is
    // invisible to the renderer.
    assert_eq!(String::from_utf8_lossy(&output.stdout), doc);

    let export = limits_artifact::read(&limits, "claude").expect("the export must be written");
    assert_eq!(export.plan.as_deref(), Some("default_claude_max_5x"));
    assert_eq!(export.account_id.as_deref(), Some("ca9db76e-0000-4000-a000-a91c5789ed3a"));
    assert_eq!(
        export.metering_regime.as_deref(),
        Some(limits_artifact::CLAUDE_METERING_REGIME),
        "one regime for every producer, or the Series splits",
    );
    // The null experiment lane wrote no window; the named lanes carry the
    // Companion's own evidence.
    assert_eq!(export.windows.len(), 2);
    assert_eq!(export.windows[0].key, "five_hour");
    assert_eq!(export.windows[0].used_pct, 16.0);
    assert_eq!(export.windows[0].evidence.limit_id.as_deref(), Some("session"));
    assert_eq!(export.windows[1].key, "seven_day");
    assert_eq!(export.windows[1].evidence.limit_id.as_deref(), Some("weekly_all"));

    // And through the same ingest the app runs, into a real migrated database.
    let mut conn = tokenledger_lib::open_db(&tmp.path().join("t.db")).unwrap();
    assert_eq!(limits_artifact::ingest(&mut conn, &limits, "claude"), Ok(2));

    // The one-shot payload capture, beside the config document.
    assert!(config.join("claude-statusline-tap-payload.json").exists());
}

#[test]
fn unchanged_windows_are_the_same_observation_and_never_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    let doc = payload(16.0, 71.0, now() + 3600);

    assert!(run(&config, &limits, &doc).status.success());
    let first = limits_artifact::read(&limits, "claude").unwrap().fetched_at;

    // A second render past the stamp's own second: identical windows must
    // keep the first stamp rather than re-dating the same observation.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(run(&config, &limits, &doc).status.success());
    assert_eq!(limits_artifact::read(&limits, "claude").unwrap().fetched_at, first);
}

#[test]
fn a_newer_artifact_is_never_regressed() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    // A live Companion fetch from the future of this run's clock second.
    limits_artifact::write(
        &limits,
        &limits_artifact::LimitsExport {
            schema: limits_artifact::SCHEMA,
            source: "claude".to_string(),
            fetched_at: now() + 60,
            windows: vec![limits_artifact::WindowExport {
                key: "five_hour".into(),
                used_pct: 55.0,
                resets_at: now() + 3600,
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .unwrap();

    assert!(run(&config, &limits, &payload(16.0, 71.0, now() + 3600)).status.success());
    let held = limits_artifact::read(&limits, "claude").unwrap();
    assert_eq!(held.windows.len(), 1);
    assert_eq!(held.windows[0].used_pct, 55.0, "the newer Artifact survives");
}

/// The incident, verbatim: a long-idle session pushed a belief whose
/// five_hour window had reset two days earlier, and the tap stamped it as
/// now. An expired window dates the whole belief — none of it may land, the
/// still-future weekly included.
#[test]
fn a_belief_with_any_expired_window_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    let doc = format!(
        r#"{{"rate_limits":{{
            "five_hour":{{"used_percentage":3,"resets_at":{}}},
            "seven_day":{{"used_percentage":18,"resets_at":{}}}}}}}"#,
        now() - 2 * 24 * 3600,
        now() + 2 * 3600,
    );
    assert!(run(&config, &limits, &doc).status.success());
    assert!(limits_artifact::read(&limits, "claude").is_none(), "a dated belief may not land");
}

/// Two live sessions render side by side; the idler one's belief is minutes
/// behind. Usage within a window only grows, so the same reset instant with a
/// lower figure is the older belief — it must not overwrite the fresher one
/// (which would ping-pong the Artifact at render frequency).
#[test]
fn an_older_belief_on_a_shared_window_never_overwrites_a_fresher_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    let resets = now() + 3 * 3600;

    assert!(run(&config, &limits, &payload(24.0, 72.0, resets)).status.success());
    // Same windows, lower session figure: an earlier belief arriving later.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(run(&config, &limits, &payload(23.0, 72.0, resets)).status.success());
    let held = limits_artifact::read(&limits, "claude").unwrap();
    assert_eq!(held.windows[0].used_pct, 24.0, "the fresher belief survives");

    // And the genuinely newer belief still lands.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(run(&config, &limits, &payload(25.0, 72.0, resets)).status.success());
    assert_eq!(limits_artifact::read(&limits, "claude").unwrap().windows[0].used_pct, 25.0);
}

#[test]
fn a_document_with_no_rate_limits_writes_nothing_and_still_renders() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    let doc = r#"{"model":{"display_name":"Fable 5"}}"#;

    let output = run(&config, &limits, doc);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), doc, "the render leg is unconditional");
    assert!(limits_artifact::read(&limits, "claude").is_none(), "nothing may be written");
}

#[test]
fn the_downstreams_exit_code_is_the_taps() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, limits) = dirs_in(tmp.path());
    let output = Command::new(BIN)
        .arg("/usr/bin/false")
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("TOKENLEDGER_LIMITS_DIR", &limits)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}
