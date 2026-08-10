//! antigravity-export — companion tool, deliberately NOT part of the scan.
//!
//! Antigravity keeps most Sessions as encrypted `<uuid>.pb` that nothing can
//! read offline (docs/source-evidence/antigravity.md). The one thing that can
//! decrypt them is Antigravity's own language server, and ADR-0013 forbids the
//! Ledger from talking to a Source, so the scan must never do this itself.
//!
//! This binary is the sanctioned way across that line: a person runs it, it
//! asks the *already-running* language server for each Session's generation
//! metadata, and writes `<uuid>.tokenledger.json` beside the `.pb`. The scan
//! then reads those files as ordinary Artifacts (ADR-0018). It starts nothing,
//! signs into nothing, and never leaves the loopback interface.
//!
//! Every address here is discovered at run time — process list for the CSRF
//! token, the OS for that process's listening ports — so it works on any
//! machine with Antigravity, not just the one it was written on.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tokenledger_lib::export_artifact::{self, ConversationExport, GenerationExport};
use tokenledger_lib::proto::{message_field, message_fields, string_field, varint_field};
use tokenledger_lib::uri::file_uri_to_path;

const RPC_GENERATIONS: &str =
    "/exa.language_server_pb.LanguageServerService/GetCascadeTrajectoryGeneratorMetadata";
const RPC_METADATA: &str = "/exa.language_server_pb.LanguageServerService/GetConversationMetadata";
const CSRF_HEADER: &str = "x-codeium-csrf-token";

fn main() {
    // Nothing here speaks TLS — the only call is plaintext HTTP/2 to loopback —
    // but reqwest's rustls stack refuses to build a client without a provider
    // installed, so install the one already in the tree. Ignore the error: it
    // only means a provider is present already.
    let _ = rustls::crypto::ring::default_provider().install_default();

    match run() {
        Ok(report) => println!("{report}"),
        Err(err) => {
            eprintln!("antigravity-export: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    // `--force` re-exports Sessions that already have a fresh export — the
    // escape hatch when the Artifact shape gains a field (e.g. `model_alias`)
    // and old exports must be regenerated to carry it.
    let force = std::env::args().skip(1).any(|a| a == "--force" || a == "-f");
    let gemini = gemini_dir()?;
    let dirs = conversation_dirs(&gemini);
    if dirs.is_empty() {
        return Err(format!(
            "no Antigravity conversations directory under {}",
            gemini.display()
        ));
    }

    let pending = pending_sessions(&dirs, force);
    if pending.is_empty() {
        return Ok(
            "nothing to do: every encrypted Session already has an export \
             (pass --force to regenerate existing exports)"
                .to_string(),
        );
    }

    // Probing needs a Session the server will actually recognise, so the first
    // pending one doubles as the handshake.
    let server = discover_server(&pending[0].id)?;
    eprintln!(
        "using language server on 127.0.0.1:{} — {} Session(s) to export",
        server.port,
        pending.len()
    );

    let (mut done, mut generations, mut failed) = (0usize, 0usize, Vec::new());
    for session in &pending {
        match export_session(&server, session) {
            Ok(count) => {
                done += 1;
                generations += count;
                if done % 10 == 0 || done == pending.len() {
                    eprintln!("  {}/{} exported", done, pending.len());
                }
            }
            Err(err) => failed.push(format!("{}: {err}", session.id)),
        }
    }

    let mut report = format!("exported {done} Session(s), {generations} generation(s)");
    if !failed.is_empty() {
        report.push_str(&format!("; {} failed", failed.len()));
        for line in failed.iter().take(5) {
            report.push_str(&format!("\n  {line}"));
        }
    }
    report.push_str("\nRescan TokenLedger to pick them up.");
    Ok(report)
}

// ---------------------------------------------------------------------------
// Locating Sessions
// ---------------------------------------------------------------------------

struct Session {
    id: String,
    /// One target per `.pb` location. The same Session id shows up under
    /// several app data dirs, and the scan reads only some of them, so writing
    /// a single copy would be a coin flip on which machine it helps.
    exports: Vec<PathBuf>,
}

fn gemini_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("GEMINI_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs_home()
        .map(|home| home.join(".gemini"))
        .ok_or_else(|| "cannot locate home directory".to_string())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The app data dirs the scan reads, and only those — they must stay in step
/// with the `antigravity` artifacts in `src/source-catalog.json`. Exporting
/// into a directory the scan never opens looks like success and changes
/// nothing, so a server reporting some other `--app_data_dir` is said out loud
/// instead: that install is outside the Ledger's reach altogether, and writing
/// a file there would only move the silence.
const SCANNED_APP_DIRS: [&str; 3] = ["antigravity", "antigravity-ide", "antigravity-cli"];

fn conversation_dirs(gemini: &Path) -> Vec<PathBuf> {
    for (_, cmdline) in language_servers() {
        if let Some(dir) = flag_value(&cmdline, "--app_data_dir") {
            if !SCANNED_APP_DIRS.contains(&dir.as_str()) {
                eprintln!(
                    "warning: a language server reports --app_data_dir {dir}, which TokenLedger \
                     does not scan — those Sessions cannot be exported"
                );
            }
        }
    }
    SCANNED_APP_DIRS
        .iter()
        .map(|app| gemini.join(app).join("conversations"))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Encrypted Sessions with no export, or whose export predates the `.pb`.
/// With `force`, every encrypted Session is pending again.
fn pending_sessions(dirs: &[PathBuf], force: bool) -> Vec<Session> {
    let mut by_id: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("pb") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let export = path.with_file_name(export_artifact::file_name(id));
            if !force && is_fresh(&export, &path) {
                continue;
            }
            // Decrypted once below, then written to every location that wants it.
            by_id.entry(id.to_string()).or_default().push(export);
        }
    }
    by_id.into_iter().map(|(id, exports)| Session { id, exports }).collect()
}

fn is_fresh(export: &Path, pb: &Path) -> bool {
    let Ok(exported) = fs::metadata(export).and_then(|m| m.modified()) else { return false };
    let Ok(encrypted) = fs::metadata(pb).and_then(|m| m.modified()) else { return false };
    exported >= encrypted
}

// ---------------------------------------------------------------------------
// Finding the running language server
// ---------------------------------------------------------------------------

struct Server {
    port: u16,
    token: String,
    http: reqwest::blocking::Client,
}

fn discover_server(probe_id: &str) -> Result<Server, String> {
    let servers = language_servers();
    if servers.is_empty() {
        return Err(
            "no Antigravity language server is running — open Antigravity and try again".into(),
        );
    }
    // One client for the whole run; cloning it shares the connection pool.
    let http = reqwest::blocking::Client::builder()
        .http2_prior_knowledge()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("cannot build the HTTP/2 client: {e}"))?;

    for (pid, cmdline) in &servers {
        let Some(token) = flag_value(cmdline, "--csrf_token") else { continue };
        for port in listening_ports(*pid) {
            let candidate = Server { port, token: token.clone(), http: http.clone() };
            // Only the plaintext gRPC port answers; the TLS and LSP ports drop
            // the connection, so a successful decode is the discriminator.
            if grpc_call(&candidate, RPC_GENERATIONS, probe_id).is_ok() {
                return Ok(candidate);
            }
        }
    }
    Err("found a language server but no port accepted the gRPC call \
         (is this a supported Antigravity build?)"
        .into())
}

/// `(pid, command line)` for every running language server process.
fn language_servers() -> Vec<(u32, String)> {
    let output = if cfg!(windows) {
        capture(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process | ForEach-Object \
                 { \"$($_.ProcessId) $($_.CommandLine)\" }",
            ],
        )
    } else {
        // `-A` is the POSIX spelling of "every process" and behaves the same on
        // macOS, Linux and BSD; `-ax` is BSD syntax that procps only tolerates.
        capture("ps", &["-Ao", "pid=,args="])
    };

    output
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let pid: u32 = pid.trim().parse().ok()?;
            rest.contains("language_server").then(|| (pid, rest.to_string()))
        })
        .collect()
}

/// Value of `--flag`, or None when the flag is absent or valueless. Antigravity
/// mixes valueless flags (`--enable_lsp`) in among the pairs, so a naive
/// "next token" scan has to reject anything that looks like another flag.
fn flag_value(cmdline: &str, flag: &str) -> Option<String> {
    let tokens: Vec<&str> = cmdline.split_whitespace().collect();
    let at = tokens.iter().position(|t| *t == flag)?;
    let value = tokens.get(at + 1)?;
    (!value.starts_with("--")).then(|| (*value).to_string())
}

fn listening_ports(pid: u32) -> Vec<u16> {
    let mut ports: Vec<u16> = if cfg!(windows) {
        let rows = capture("netstat", &["-ano"]).unwrap_or_default();
        let owned: Vec<&str> = rows
            .lines()
            .filter(|l| l.split_whitespace().next_back() == Some(pid.to_string().as_str()))
            .collect();
        // netstat localises its state column ("LISTENING" is "ABHÖREN" on a
        // German install), so the word cannot be the filter. Prefer the rows
        // that look like listeners, then fall back to every row this process
        // owns — a few extra candidates only cost one failed probe each.
        let listening: Vec<u16> =
            owned.iter().filter(|l| l.contains("LISTEN")).filter_map(|l| port_in_line(l)).collect();
        if listening.is_empty() {
            owned.iter().filter_map(|l| port_in_line(l)).collect()
        } else {
            listening
        }
    } else {
        let lsof = capture(
            "lsof",
            &["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()],
        );
        match lsof {
            Some(text) if !text.trim().is_empty() => {
                text.lines().skip(1).filter_map(port_in_line).collect()
            }
            // lsof is not installed everywhere; ss covers most of Linux.
            _ => capture("ss", &["-ltnp"])
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains(&format!("pid={pid},")))
                .filter_map(port_in_line)
                .collect(),
        }
    };
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// First `:<port>` on a line — the *local* address in lsof, netstat and ss
/// alike. Taking the last instead would pick up netstat's remote `0.0.0.0:0`
/// column and report port 0 on Windows.
fn port_in_line(line: &str) -> Option<u16> {
    line.split_whitespace()
        .filter_map(|field| field.rsplit_once(':'))
        .filter_map(|(_, port)| {
            port.trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<u16>().ok()
        })
        .find(|port| *port != 0)
}

fn capture(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// gRPC over HTTP/2, via curl
// ---------------------------------------------------------------------------

/// One unary call, over plaintext HTTP/2 to loopback. Done in-process rather
/// than by shelling out to curl: `ureq` cannot speak h2, and Windows' bundled
/// `curl.exe` is a Schannel build that has shipped without HTTP/2 support — so
/// depending on it would have made this a Unix-only tool in practice.
fn grpc_call(server: &Server, rpc: &str, conversation_id: &str) -> Result<Vec<u8>, String> {
    let response = server
        .http
        .post(format!("http://127.0.0.1:{}{rpc}", server.port))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header(CSRF_HEADER, &server.token)
        .body(frame(&length_delimited(1, conversation_id.as_bytes())))
        .send()
        .map_err(|e| format!("gRPC call failed: {e}"))?;

    let body = response.bytes().map_err(|e| format!("cannot read response: {e}"))?;
    unframe(&body).ok_or_else(|| "empty or truncated gRPC response".to_string())
}

fn length_delimited(field: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, field << 3 | 2);
    put_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// gRPC length-prefixed framing: compression flag + big-endian u32 length.
fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8];
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// Concatenate every frame's payload (unary calls send one, but be tolerant).
fn unframe(body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 5 <= body.len() {
        let len = u32::from_be_bytes(body[pos + 1..pos + 5].try_into().ok()?) as usize;
        pos += 5;
        let end = pos.checked_add(len)?;
        if end > body.len() {
            break;
        }
        out.extend_from_slice(&body[pos..end]);
        pos = end;
    }
    (!out.is_empty()).then_some(out)
}

// ---------------------------------------------------------------------------
// Decoding one Session
// ---------------------------------------------------------------------------

fn export_session(server: &Server, session: &Session) -> Result<usize, String> {
    let body = grpc_call(server, RPC_GENERATIONS, &session.id)?;

    let mut generations: Vec<GenerationExport> = Vec::new();
    let mut model: Option<String> = None;
    for entry in message_fields(&body, 1) {
        // Recorded once per Session at most, so keep the first one seen.
        if model.is_none() {
            model = message_field(entry, 3)
                .and_then(|planner| string_field(planner, 28))
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string);
        }
        let Some(chat_model) = message_field(entry, 1) else { continue };
        let Some(usage) = message_field(chat_model, 4) else { continue };

        let reasoning = varint_field(usage, 9).unwrap_or(0);
        let response = varint_field(usage, 10).unwrap_or(0);
        let output = varint_field(usage, 3).unwrap_or(reasoning + response);
        let input = varint_field(usage, 2).unwrap_or(0);
        let cache_read = varint_field(usage, 5).unwrap_or(0);
        let cache_write = varint_field(usage, 4).unwrap_or(0);
        if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
            continue;
        }

        let timestamp = message_field(chat_model, 9)
            .and_then(|start| message_field(start, 4))
            .and_then(|created| varint_field(created, 1))
            .unwrap_or(0);

        // The counts are protobuf varints (u64) but the Ledger keeps signed
        // totals; saturate rather than wrap, so a nonsense value stays large
        // instead of turning negative.
        let as_i64 = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        generations.push(GenerationExport {
            response_id: string_field(usage, 11).map(str::to_string),
            ts: as_i64(timestamp),
            model_enum: varint_field(chat_model, 3),
            // The per-generation wire alias: the true Model of the request,
            // where the Session-level label is only the picker default. Old
            // exports predate it and the reader falls back, so recording it
            // is what finally names the MODEL_PLACEHOLDER enums.
            model_alias: string_field(chat_model, 19)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            input: as_i64(input),
            output: as_i64(output),
            cache_read: as_i64(cache_read),
            cache_write: as_i64(cache_write),
            thinking: as_i64(reasoning),
        });
    }

    let project = grpc_call(server, RPC_METADATA, &session.id)
        .ok()
        .and_then(|meta| workspace_path(&meta));

    let exported = generations.len();
    // The very types the adapter deserializes, so a field cannot be spelled one
    // way here and another there.
    let document = ConversationExport {
        schema: export_artifact::SCHEMA,
        conversation_id: session.id.clone(),
        model,
        project,
        generations,
    };
    let rendered = serde_json::to_string_pretty(&document)
        .map_err(|e| format!("cannot render export: {e}"))?;
    // Write-then-rename, because the app rescans on a timer: a plain write can
    // be read half-finished and reported as a corrupt export. Rename inside the
    // same directory is atomic, so a scan sees the old file or the new one.
    for target in &session.exports {
        let staged = target.with_extension("json.part");
        fs::write(&staged, &rendered)
            .map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
        fs::rename(&staged, target)
            .map_err(|e| format!("cannot publish {}: {e}", target.display()))?;
    }
    Ok(exported)
}

/// The workspace a Session belongs to. ponytail: the metadata response nests it
/// several messages deep, so rather than pin field numbers that Antigravity is
/// free to renumber, take the first `file://` URI in the payload — there is
/// only ever one workspace tree in it.
fn workspace_path(body: &[u8]) -> Option<String> {
    const PREFIX: &[u8] = b"file://";
    body.windows(PREFIX.len())
        .position(|window| window == PREFIX)
        .and_then(|at| {
            let tail = &body[at..];
            let end = tail
                .iter()
                .position(|b| !matches!(b, 0x20..=0x7e))
                .unwrap_or(tail.len());
            std::str::from_utf8(&tail[..end]).ok()
        })
        .and_then(file_uri_to_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The scan reads `antigravity/` and `antigravity-cli/` but not
    // `antigravity-ide/`, so a Session present in more than one of them needs an
    // export in each — writing just one is a bet on directory ordering.
    #[test]
    fn a_session_in_several_app_dirs_gets_an_export_in_each() {
        let root = tempfile::tempdir().unwrap();
        let ide = root.path().join("antigravity-ide/conversations");
        let app = root.path().join("antigravity/conversations");
        fs::create_dir_all(&ide).unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(ide.join("dup.pb"), b"sealed").unwrap();
        fs::write(app.join("dup.pb"), b"sealed").unwrap();
        fs::write(app.join("solo.pb"), b"sealed").unwrap();

        let pending = pending_sessions(&[app.clone(), ide.clone()], false);
        assert_eq!(pending.len(), 2, "one entry per Session id, not per file");
        let dup = pending.iter().find(|s| s.id == "dup").unwrap();
        assert_eq!(dup.exports.len(), 2, "both copies get an export");
        assert!(dup.exports.contains(&app.join(export_artifact::file_name("dup"))));
        assert!(dup.exports.contains(&ide.join(export_artifact::file_name("dup"))));
        assert_eq!(pending.iter().find(|s| s.id == "solo").unwrap().exports.len(), 1);
    }

    #[test]
    fn an_export_newer_than_its_pb_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("done.pb"), b"sealed").unwrap();
        fs::write(dir.path().join(export_artifact::file_name("done")), b"{}").unwrap();
        fs::write(dir.path().join("todo.pb"), b"sealed").unwrap();

        let ids: Vec<String> =
            pending_sessions(&[dir.path().to_path_buf()], false).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["todo".to_string()]);
        // --force regenerates even Sessions with a fresh export.
        let forced: Vec<String> =
            pending_sessions(&[dir.path().to_path_buf()], true).into_iter().map(|s| s.id).collect();
        assert_eq!(forced, vec!["done".to_string(), "todo".to_string()]);
    }

    #[test]
    fn flag_value_skips_valueless_flags() {
        let cmdline = "language_server --enable_lsp --csrf_token abc123 --lsp_port 1";
        assert_eq!(flag_value(cmdline, "--csrf_token"), Some("abc123".into()));
        // `--enable_lsp` takes no value; its "value" is the next flag.
        assert_eq!(flag_value(cmdline, "--enable_lsp"), None);
        assert_eq!(flag_value(cmdline, "--missing"), None);
    }

    #[test]
    fn ports_come_off_real_tool_output() {
        assert_eq!(
            port_in_line("language_ 96072 me 7u IPv4 0x1 0t0 TCP 127.0.0.1:54394 (LISTEN)"),
            Some(54394)
        );
        // netstat puts a remote `0.0.0.0:0` column after the local address.
        assert_eq!(
            port_in_line("  TCP    127.0.0.1:54394    0.0.0.0:0    LISTENING   96072"),
            Some(54394)
        );
        assert_eq!(
            port_in_line("LISTEN 0 4096 127.0.0.1:54394 0.0.0.0:* users:((\"ls\",pid=96072,fd=7))"),
            Some(54394)
        );
        assert_eq!(port_in_line("[::1]:54394 LISTEN"), Some(54394));
        assert_eq!(port_in_line("no ports here"), None);
    }

    #[test]
    fn framing_round_trips() {
        let message = length_delimited(1, b"abc");
        assert_eq!(message, vec![0x0a, 0x03, b'a', b'b', b'c']);
        assert_eq!(unframe(&frame(&message)).unwrap(), message);
        assert_eq!(unframe(&[0, 0, 0]), None);
    }



    #[test]
    fn workspace_path_is_decoded() {
        let mut body = b"\x00\x01junk".to_vec();
        body.extend_from_slice(b"file:///Users/me/My%20Code\x00trailing");
        assert_eq!(workspace_path(&body).as_deref(), Some("/Users/me/My Code"));
        assert_eq!(workspace_path(b"no uri here"), None);
    }

}
