//! TOKL-20 integration seam: the real `claude-limits` binary as a separate
//! process — the same boundary the app's sidecar spawn crosses — with its
//! config dir, limits dir, and (debug-only) vendor URL injected by env, then
//! the written Export Artifact carried through the SAME schema-checked ingest
//! the app uses, into a real migrated database.
//!
//! What stays out on purpose: spawning through the Tauri shell capability
//! (headless CI cannot run the app; that wiring is covered by the recorded
//! manual verification) and the vendor itself (the URL override exists only in
//! debug builds, which is what `cargo test` builds).
//!
//! macOS note: on a developer machine the binary's keystore probe may find a
//! real Claude sign-in before the temp `.credentials.json` fallback. Every
//! live-path test here aims the fetch at a local scripted server, so which
//! credential is presented never decides an outcome — the scripted answers do.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

use tokenledger_lib::limits_artifact;

const BIN: &str = env!("CARGO_BIN_EXE_claude-limits");

/// A vendor answer nobody can reach: connection refused, instantly. The
/// fresh-cache test uses it so a regression that fetches anyway fails loudly
/// instead of silently reaching the real vendor.
const DEAD_VENDOR: &str = "http://127.0.0.1:9";

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// Claude Code's config document — the SAME captured fixture the Companion's
/// unit seam parses, so the two seams can never drift — re-stamped as fetched
/// at `fetched_at_ms`.
fn write_cache_document(dir: &Path, fetched_at_ms: i64) {
    let document = include_str!("fixtures/claude/cached-config-document.json")
        .replace("1787256759360", &fetched_at_ms.to_string());
    assert!(document.contains(&fetched_at_ms.to_string()), "the fixture's stamp must be replaced");
    std::fs::write(dir.join(".claude.json"), document).unwrap();
}

/// A credential file for the live path — what the binary falls back to where
/// the platform keystore holds nothing (all of CI).
fn write_credentials(dir: &Path) {
    std::fs::write(
        dir.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"sk-test-token",
            "scopes":["user:inference","user:profile"],"rateLimitTier":"default_claude_max_5x"}}"#,
    )
    .unwrap();
}

/// The scripted vendor: each canned response on its own connection, then gone.
/// Same shape as the Companion's own unit-test server.
fn serve(responses: Vec<String>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for response in responses {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut [0u8; 1024]);
            socket.write_all(response.as_bytes()).unwrap();
        }
    });
    url
}

const REFUSED: &str =
    "HTTP/1.1 429 Too Many Requests\r\nretry-after: 1\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";
const SIGNED_OUT: &str =
    "HTTP/1.1 401 Unauthorized\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";

fn answered(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len(),
    )
}

fn run_companion(config_dir: &Path, limits_dir: &Path, vendor_url: &str) -> Output {
    std::process::Command::new(BIN)
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("TOKENLEDGER_LIMITS_DIR", limits_dir)
        .env("TOKENLEDGER_CLAUDE_USAGE_URL", vendor_url)
        .output()
        .expect("the companion binary must run")
}

#[test]
fn a_fresh_cache_answers_fully_locally_and_lands_through_the_real_ingest() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    // Two minutes old: inside the gate, and far enough from `now` that a
    // regression stamping the export with the current time is caught below.
    let fetched_at_ms = now_ms() - 120_000;
    write_cache_document(&config, fetched_at_ms);
    // No credential file, and the vendor unreachable: this path must need
    // neither, so an accidental fetch or credential demand fails the run.
    let output = run_companion(&config, &limits, DEAD_VENDOR);
    assert!(
        output.status.success(),
        "a fresh cache must answer: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    // The Artifact, through the same schema-checked reader the scan uses.
    let export = limits_artifact::read(&limits, "claude").expect("the export must be written");
    assert_eq!(export.fetched_at, fetched_at_ms / 1000, "the cache's own stamp, never now()");
    assert_eq!(export.account_id.as_deref(), Some("ca9db76e-0000-4000-a000-a91c5789ed3a"));
    assert_eq!(export.plan.as_deref(), Some("default_claude_max_5x"));

    // And through the same ingest the app runs, into a real migrated database.
    let mut conn = tokenledger_lib::open_db(&tmp.path().join("t.db")).unwrap();
    assert_eq!(limits_artifact::ingest(&mut conn, &limits, "claude"), Ok(3));
    let rows: Vec<(String, f64, i64, String, Option<String>, Option<String>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT window_key, used_pct, observed_at, via, account_id, limit_id \
                 FROM limit_readings WHERE source = 'claude' ORDER BY window_key",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let uuid = Some("ca9db76e-0000-4000-a000-a91c5789ed3a".to_string());
    assert_eq!(
        rows,
        vec![
            ("five_hour".into(), 36.0, fetched_at_ms / 1000, "live".into(), uuid.clone(), Some("session".into())),
            ("seven_day".into(), 31.0, fetched_at_ms / 1000, "live".into(), uuid.clone(), Some("weekly_all".into())),
            // The model-scoped window renders but proves no Limit identity —
            // same as on the live path.
            ("seven_day_fable".into(), 41.0, fetched_at_ms / 1000, "live".into(), uuid, None),
        ],
    );

    // Re-serving an unchanged cache is the SAME observation: the rewritten
    // Artifact re-ingests as a no-op, never a duplicate Reading — and this is
    // the highest-frequency path there is (every page open inside the gate).
    assert!(run_companion(&config, &limits, DEAD_VENDOR).status.success());
    assert_eq!(limits_artifact::ingest(&mut conn, &limits, "claude"), Ok(0));
    let held: i64 = conn
        .query_row("SELECT COUNT(*) FROM limit_readings WHERE source = 'claude'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(held, 3, "append-only means no duplicates, not more rows");
}

/// The gate half of the never-regress rule: a cache inside the freshness gate
/// still yields to the live fetch when the Artifact already holds something
/// newer — answering locally there would overwrite a newer reading with an
/// older one.
#[test]
fn a_fresh_cache_behind_the_artifact_yields_to_the_live_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    // Two minutes old — inside the gate — but the Artifact is one minute old.
    write_cache_document(&config, now_ms() - 120_000);
    write_credentials(&config);
    limits_artifact::write(
        &limits,
        &limits_artifact::LimitsExport {
            schema: limits_artifact::SCHEMA,
            source: "claude".to_string(),
            fetched_at: now_ms() / 1000 - 60,
            ..Default::default()
        },
    )
    .unwrap();

    let started = now_ms() / 1000;
    let url = serve(vec![answered(
        r#"{"five_hour":{"utilization":77.0,"resets_at":1786503900}}"#,
    )]);
    let output = run_companion(&config, &limits, &url);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    // The live answer won — the in-gate cache did not answer locally.
    let export = limits_artifact::read(&limits, "claude").expect("the export must be written");
    assert_eq!(export.windows.len(), 1);
    assert_eq!(export.windows[0].used_pct, 77.0);
    assert!(export.fetched_at >= started, "a live fetch is stamped with its own moment");
}

#[test]
fn a_stale_cache_yields_to_the_live_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    write_cache_document(&config, now_ms() - 3_600_000);
    write_credentials(&config);
    let started = now_ms() / 1000;
    let url = serve(vec![answered(
        r#"{"five_hour":{"utilization":55.0,"resets_at":1786503900}}"#,
    )]);

    let output = run_companion(&config, &limits, &url);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let export = limits_artifact::read(&limits, "claude").expect("the export must be written");
    // The live answer won: its figure, and a fetch-time stamp.
    assert_eq!(export.windows.len(), 1);
    assert_eq!(export.windows[0].used_pct, 55.0);
    assert!(export.fetched_at >= started, "a live fetch is stamped with its own moment");
}

#[test]
fn a_rate_limited_fetch_falls_back_to_the_stale_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    let fetched_at_ms = now_ms() - 3_600_000;
    write_cache_document(&config, fetched_at_ms);
    write_credentials(&config);
    // Both the try and its one retry refused — the verdict that used to be the
    // card's whole answer.
    let url = serve(vec![REFUSED.to_string(), REFUSED.to_string()]);

    let output = run_companion(&config, &limits, &url);
    assert!(
        output.status.success(),
        "a held cache must answer a 429: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let export = limits_artifact::read(&limits, "claude").expect("the export must be written");
    assert_eq!(export.fetched_at, fetched_at_ms / 1000, "an hour old and says so");
    assert_eq!(export.windows.len(), 3);
}

/// The incident this rule comes from: Claude Code stopped refreshing its
/// cache for days while live checks kept landing, then a 429 arrived. The
/// fallback delivered the days-old cache, overwriting the newer Artifact and
/// exiting 0 — so the card sat stale with no refusal to explain why. A cache
/// from behind the Artifact must not answer a 429: the refusal is the verdict,
/// and the Artifact keeps the newest reading.
#[test]
fn a_rate_limited_fetch_never_regresses_the_artifact_to_an_older_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    // The cache is an hour old; the Artifact holds a LIVE reading from after
    // it — written the way a real earlier run wrote it.
    write_cache_document(&config, now_ms() - 3_600_000);
    write_credentials(&config);
    let held = limits_artifact::LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "claude".to_string(),
        fetched_at: now_ms() / 1000 - 60,
        windows: vec![limits_artifact::WindowExport {
            key: "five_hour".into(),
            window_minutes: Some(300),
            used_pct: 55.0,
            resets_at: 1_786_503_900,
            ..Default::default()
        }],
        ..Default::default()
    };
    limits_artifact::write(&limits, &held).unwrap();

    let url = serve(vec![REFUSED.to_string(), REFUSED.to_string()]);
    let output = run_companion(&config, &limits, &url);
    assert!(!output.status.success(), "with nothing newer to say, the 429 is the verdict");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "claude-limits: the vendor rate-limited this check (429) — try again in a minute",
    );

    // The Artifact still holds the newer live reading, untouched.
    let export = limits_artifact::read(&limits, "claude").expect("the export must survive");
    assert_eq!(export.fetched_at, held.fetched_at);
    assert_eq!(export.windows.len(), 1);
    assert_eq!(export.windows[0].used_pct, 55.0);
}

/// The equal-stamp corner of the same rule: an Artifact that IS the cache
/// (an earlier run inside the gate delivered it, then the cache aged past the
/// gate unrefreshed) means the fallback has no news either — re-delivering it
/// would exit 0 having changed nothing, and the card would sit silently stale.
#[test]
fn a_rate_limited_fetch_does_not_redeliver_the_cache_it_already_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    // Six minutes old — past the gate — and the Artifact carries the SAME
    // stamp: exactly what an earlier in-gate run of this binary left behind.
    let fetched_at_ms = now_ms() - 360_000;
    write_cache_document(&config, fetched_at_ms);
    write_credentials(&config);
    limits_artifact::write(
        &limits,
        &limits_artifact::LimitsExport {
            schema: limits_artifact::SCHEMA,
            source: "claude".to_string(),
            fetched_at: fetched_at_ms / 1000,
            ..Default::default()
        },
    )
    .unwrap();

    let url = serve(vec![REFUSED.to_string(), REFUSED.to_string()]);
    let output = run_companion(&config, &limits, &url);
    assert!(
        !output.status.success(),
        "an already-delivered cache answers nothing: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rate-limited"), "{stderr}");
}

#[test]
fn a_sign_in_refusal_never_borrows_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    // A stale cache is HELD — and must still not answer for a dead login.
    write_cache_document(&config, now_ms() - 3_600_000);
    write_credentials(&config);
    let url = serve(vec![SIGNED_OUT.to_string()]);

    let output = run_companion(&config, &limits, &url);
    assert!(!output.status.success(), "a 401 is a failure, cache or no cache");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not signed in"), "{stderr}");
    assert!(limits_artifact::read(&limits, "claude").is_none(), "nothing may be written");
}

#[test]
fn no_cache_and_a_rate_limited_fetch_is_still_the_honest_error() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let limits = tmp.path().join("limits");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&limits).unwrap();

    write_credentials(&config);
    let url = serve(vec![REFUSED.to_string(), REFUSED.to_string()]);

    let output = run_companion(&config, &limits, &url);
    assert!(!output.status.success(), "with nothing to fall back on, the 429 is the verdict");
    // Verbatim: the page classifies failures by their wording, so the exact
    // line — never wearing the signed-out face — is the contract.
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "claude-limits: the vendor rate-limited this check (429) — try again in a minute",
    );
    assert!(limits_artifact::read(&limits, "claude").is_none(), "nothing may be written");
}
