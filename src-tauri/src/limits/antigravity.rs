// Google Antigravity — quota lives behind the RUNNING IDE's local
// language-server API (self-signed HTTPS on a dynamic port, CSRF token in
// the process args). ps → detect process + csrf; lsof → candidate ports;
// probe with GetUnleashData; then RetrieveUserQuotaSummary, falling back to
// GetUserStatus / GetCommandModelConfigs (older servers / agy CLI).
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{clamp_percent, parse_any_ts, FetchErr, LimitWindow, ToolLimits};

const SVC: &str = "/exa.language_server_pb.LanguageServerService";
const NOT_RUNNING_MSG: &str =
    "Antigravity IDE is not running. Launch Antigravity to see usage limits.";

pub struct ProcessInfo {
    pub pid: i64,
    pub csrf_token: Option<String>,
    pub extension_port: Option<u16>,
}

pub fn parse_process_line(line: &str) -> Option<(i64, String)> {
    let trimmed = line.trim();
    let (pid, rest) = trimmed.split_once(char::is_whitespace)?;
    let pid = pid.parse::<i64>().ok()?;
    Some((pid, rest.trim_start().to_string()))
}

fn first_token(command: &str) -> &str {
    let t = command.trim_start();
    match t.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let inner = &t[1..];
            inner.split(q).next().unwrap_or(inner)
        }
        _ => t.split_whitespace().next().unwrap_or(""),
    }
}

pub fn is_antigravity_command_line(command: &str) -> bool {
    let lower = command.to_lowercase();
    let exe = first_token(command)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    // agy CLI binary is itself the server.
    if exe == "agy" || exe == "agy.exe" {
        return true;
    }
    // IDE language_server — require Antigravity markers so sibling Codeium
    // products (Windsurf etc.) are not misidentified.
    let is_lang_server = exe.starts_with("language_server");
    let has_marker = (lower.contains("--app_data_dir") && lower.contains("antigravity"))
        || lower.contains("/antigravity/")
        || lower.contains("/antigravity.app/")
        || lower.contains("\\antigravity\\")
        || lower.contains("--override_ide_name=antigravity")
        || lower.contains("--override_ide_name antigravity");
    is_lang_server && has_marker
}

pub fn extract_flag(command: &str, flag: &str) -> Option<String> {
    let idx = command.find(flag)?;
    let rest = &command[idx + flag.len()..];
    let rest = rest.trim_start_matches(['=', ' ', '\t']);
    let val: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    (!val.is_empty()).then_some(val)
}

fn detect_process() -> Result<Option<ProcessInfo>, String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let Some((pid, command)) = parse_process_line(line) else { continue };
        if !is_antigravity_command_line(&command) {
            continue;
        }
        let csrf_token = extract_flag(&command, "--csrf_token");
        let extension_port = extract_flag(&command, "--extension_server_port")
            .and_then(|p| p.parse::<u16>().ok());
        return Ok(Some(ProcessInfo { pid, csrf_token, extension_port }));
    }
    Ok(None)
}

pub fn parse_listening_ports(output: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = Vec::new();
    for line in output.lines() {
        if !line.contains("(LISTEN)") {
            continue;
        }
        // "... TCP 127.0.0.1:42117 (LISTEN)" → take the :port right before (LISTEN)
        for part in line.split_whitespace() {
            if !part.contains(':') {
                continue; // skip bare tokens (pid, fd, ...) — only host:port has a port
            }
            if let Some(rest) = part.rsplit(':').next() {
                if let Ok(p) = rest.parse::<u16>() {
                    if !ports.contains(&p) {
                        ports.push(p);
                    }
                }
            }
        }
    }
    ports.sort_unstable();
    ports
}

fn list_ports(pid: i64) -> Result<Vec<u16>, String> {
    let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .iter()
        .find(|p| Path::new(p).exists())
        .ok_or("Antigravity port detection needs lsof. Install it, then retry.")?;
    let out = std::process::Command::new(lsof)
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    let ports = parse_listening_ports(&String::from_utf8_lossy(&out.stdout));
    if ports.is_empty() {
        return Err("Antigravity is running but not exposing ports yet. Try again in a few seconds.".into());
    }
    Ok(ports)
}

/// Local agent accepting the IDE's self-signed cert. Local-only (127.0.0.1).
fn local_agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .tls_connector(Arc::new(tls))
        .build())
}

fn call_local(
    agent: &ureq::Agent,
    scheme: &str,
    port: u16,
    method: &str,
    body: &Value,
    csrf: Option<&str>,
) -> Result<Value, String> {
    let url = format!("{scheme}://127.0.0.1:{port}{SVC}/{method}");
    let mut req = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1");
    if let Some(t) = csrf {
        req = req.set("X-Codeium-Csrf-Token", t);
    }
    match req.send_json(body.clone()) {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn default_body() -> Value {
    json!({ "metadata": {
        "ideName": "antigravity", "extensionName": "antigravity",
        "ideVersion": "unknown", "locale": "en",
    }})
}

fn unleash_body() -> Value {
    json!({ "context": { "properties": {
        "devMode": "false", "extensionVersion": "unknown",
        "hasAnthropicModelAccess": "true", "ide": "antigravity",
        "ideVersion": "unknown", "installationId": "tokenledger",
        "language": "UNSPECIFIED", "os": "macos",
        "requestedModelId": "MODEL_UNSPECIFIED",
    }}})
}

fn code_ok(body: &Value) -> bool {
    match body.get("code") {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_i64() == Some(0),
        Some(Value::String(s)) => {
            let l = s.to_lowercase();
            l == "ok" || l == "success" || l == "0"
        }
        _ => false,
    }
}

pub fn normalize_quota_summary(body: &Value) -> Result<Vec<LimitWindow>, String> {
    if !code_ok(body) {
        return Err(format!("Antigravity API error: {}", body.get("code").unwrap_or(&Value::Null)));
    }
    let groups = body
        .pointer("/response/groups")
        .and_then(Value::as_array)
        .filter(|g| !g.is_empty())
        .ok_or("Could not parse Antigravity quota summary: no groups.")?;
    let mut buckets: std::collections::HashMap<&str, &Value> = std::collections::HashMap::new();
    for g in groups {
        if let Some(bs) = g.get("buckets").and_then(Value::as_array) {
            for b in bs {
                if let Some(id) = b.get("bucketId").and_then(Value::as_str) {
                    buckets.insert(id, b);
                }
            }
        }
    }
    let win = |id: &str, label: &str| -> Option<LimitWindow> {
        let b = buckets.get(id)?;
        let rf = b.get("remainingFraction").and_then(Value::as_f64)?;
        Some(LimitWindow {
            label: label.into(),
            used_percent: clamp_percent(100.0 - rf * 100.0),
            resets_at_ts: b.get("resetTime").and_then(parse_any_ts),
        })
    };
    let out: Vec<LimitWindow> = [
        win("3p-weekly", "Cl 7d"),
        win("3p-5h", "Cl 5h"),
        win("gemini-weekly", "Gm 7d"),
        win("gemini-5h", "Gm 5h"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if out.is_empty() {
        // Known bucketIds all missing (upstream rename) → treat as parse
        // failure so the caller falls back to GetUserStatus.
        return Err("Could not parse Antigravity quota summary: no known buckets matched.".into());
    }
    Ok(out)
}

struct ModelQuota {
    text: String, // label + model id, lowercased, for family classification
    remaining_fraction: Option<f64>,
    reset_ts: Option<i64>,
}

fn parse_model_configs(configs: Option<&Value>) -> Vec<ModelQuota> {
    let Some(arr) = configs.and_then(Value::as_array) else { return Vec::new() };
    arr.iter()
        .filter_map(|c| {
            let quota = c.get("quotaInfo")?;
            let label = c.get("label").and_then(Value::as_str).unwrap_or("");
            let model = c.pointer("/modelOrAlias/model").and_then(Value::as_str).unwrap_or("");
            Some(ModelQuota {
                text: format!("{label} {model}").to_lowercase(),
                remaining_fraction: quota.get("remainingFraction").and_then(Value::as_f64),
                reset_ts: quota.get("resetTime").and_then(parse_any_ts),
            })
        })
        .collect()
}

fn family(m: &ModelQuota) -> &'static str {
    if m.text.contains("claude") {
        "claude"
    } else if m.text.contains("gemini") && m.text.contains("pro") {
        "gemini_pro"
    } else if m.text.contains("gemini") && m.text.contains("flash") {
        "gemini_flash"
    } else {
        "unknown"
    }
}

fn is_chat_model(m: &ModelQuota) -> bool {
    !(m.text.contains("lite") || m.text.contains("autocomplete") || m.text.contains("tab_"))
}

pub fn normalize_user_status(
    body: &Value,
    fallback_to_configs: bool,
) -> Result<(Option<String>, Vec<LimitWindow>), String> {
    if !code_ok(body) {
        return Err(format!("Antigravity API error: {}", body.get("code").unwrap_or(&Value::Null)));
    }
    let user_status = body.get("userStatus");
    let configs = if fallback_to_configs {
        body.get("clientModelConfigs")
    } else {
        user_status.and_then(|u| u.pointer("/cascadeModelConfigData/clientModelConfigs"))
    };
    let all = parse_model_configs(configs);
    if all.is_empty() {
        return Err("Could not parse Antigravity quota: no quota models available.".into());
    }
    let chat: Vec<&ModelQuota> = all.iter().filter(|m| is_chat_model(m)).collect();
    let models: Vec<&ModelQuota> = if chat.is_empty() { all.iter().collect() } else { chat };
    let claude: Vec<&&ModelQuota> = models.iter().filter(|m| family(m) == "claude").collect();
    let gemini: Vec<&&ModelQuota> = models
        .iter()
        .filter(|m| matches!(family(m), "gemini_pro" | "gemini_flash"))
        .collect();
    // Most-used (min remaining → the weekly quota) and least-used (max
    // remaining → the 5h rolling quota) per family.
    fn pick<'a>(
        list: &[&&'a ModelQuota],
        min: bool,
    ) -> Option<&'a ModelQuota> {
        let mut with_rf: Vec<&&ModelQuota> =
            list.iter().filter(|m| m.remaining_fraction.is_some()).copied().collect();
        if with_rf.is_empty() {
            return list.first().copied().copied();
        }
        with_rf.sort_by(|a, b| {
            a.remaining_fraction
                .partial_cmp(&b.remaining_fraction)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let m = if min { with_rf.first() } else { with_rf.last() };
        m.copied().copied()
    }
    let win = |m: Option<&ModelQuota>, label: &str| -> Option<LimitWindow> {
        let m = m?;
        let rf = m.remaining_fraction.unwrap_or(0.0);
        Some(LimitWindow {
            label: label.into(),
            used_percent: clamp_percent(100.0 - rf * 100.0),
            resets_at_ts: m.reset_ts,
        })
    };
    let all_ref: Vec<&&ModelQuota> = models.iter().collect();
    let windows: Vec<LimitWindow> = [
        win(if claude.is_empty() { pick(&all_ref, true) } else { pick(&claude, true) }, "Cl 7d"),
        win(if claude.is_empty() { None } else { pick(&claude, false) }, "Cl 5h"),
        win(if gemini.is_empty() { None } else { pick(&gemini, true) }, "Gm 7d"),
        win(if gemini.is_empty() { None } else { pick(&gemini, false) }, "Gm 5h"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let plan = user_status
        .map(|u| {
            [
                "/planStatus/planInfo/planDisplayName",
                "/planStatus/planInfo/displayName",
                "/planStatus/planInfo/productName",
                "/planStatus/planInfo/planName",
                "/planStatus/planInfo/planShortName",
            ]
            .iter()
            .find_map(|p| u.pointer(p).and_then(Value::as_str))
            .map(String::from)
        })
        .unwrap_or(None);
    Ok((plan, windows))
}

fn has_install_evidence(home: &Path) -> bool {
    ["antigravity", "antigravity-ide", "antigravity-cli"]
        .iter()
        .any(|d| home.join(".gemini").join(d).exists())
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let process = match detect_process() {
        Ok(p) => p,
        Err(e) => return Err(FetchErr::from(e)),
    };
    let Some(info) = process else {
        if !has_install_evidence(home) {
            return Ok(ToolLimits::not_configured("antigravity"));
        }
        // Installed but not running → orchestrator's with_fallback serves the
        // disk cache; this message shows only when no cache exists.
        return Err(FetchErr::from(NOT_RUNNING_MSG));
    };
    let ports = list_ports(info.pid).map_err(FetchErr::from)?;
    let agent = local_agent().map_err(FetchErr::from)?;
    let csrf = info.csrf_token.as_deref();
    let mut working: Option<(u16, &str)> = None;
    for port in &ports {
        if call_local(&agent, "https", *port, "GetUnleashData", &unleash_body(), csrf).is_ok() {
            working = Some((*port, "https"));
            break;
        }
        // agy CLI serves plain HTTP with no CSRF.
        if csrf.is_none()
            && call_local(&agent, "http", *port, "GetUnleashData", &unleash_body(), None).is_ok()
        {
            working = Some((*port, "http"));
            break;
        }
    }
    let Some((port, scheme)) = working else {
        return Err(FetchErr::from("Antigravity port detection failed: no working API port found"));
    };
    // Preferred: quota summary (newest servers).
    if let Ok(body) = call_local(&agent, scheme, port, "RetrieveUserQuotaSummary", &default_body(), csrf) {
        if let Ok(windows) = normalize_quota_summary(&body) {
            return Ok(ToolLimits::live("antigravity", None, windows, now_ts));
        }
    }
    // Fallback: GetUserStatus (IDE), then GetCommandModelConfigs (agy CLI).
    match call_local(&agent, scheme, port, "GetUserStatus", &default_body(), csrf) {
        Ok(body) => {
            let (plan, windows) = normalize_user_status(&body, false).map_err(FetchErr::from)?;
            Ok(ToolLimits::live("antigravity", plan, windows, now_ts))
        }
        Err(_) => {
            let fallback_port = info.extension_port.unwrap_or(port);
            let fallback_scheme = if fallback_port == port {
                if scheme == "https" { "http" } else { "https" }
            } else {
                "http"
            };
            let body = call_local(
                &agent,
                fallback_scheme,
                fallback_port,
                "GetCommandModelConfigs",
                &default_body(),
                csrf,
            )
            .map_err(FetchErr::from)?;
            let (plan, windows) = normalize_user_status(&body, true).map_err(FetchErr::from)?;
            Ok(ToolLimits::live("antigravity", plan, windows, now_ts))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_process_lines() {
        let (pid, cmd) = parse_process_line("  423  /Applications/Antigravity.app/x language_server_macos --csrf_token abc").unwrap();
        assert_eq!(pid, 423);
        assert!(cmd.contains("language_server_macos"));
        assert!(parse_process_line("garbage").is_none());
    }

    #[test]
    fn identifies_antigravity_commands() {
        assert!(is_antigravity_command_line(
            "/Applications/Antigravity.app/Contents/language_server_macos_arm --app_data_dir /x/antigravity --csrf_token t"
        ));
        assert!(is_antigravity_command_line("/usr/local/bin/agy serve"));
        assert!(!is_antigravity_command_line("vim /tmp/agy"));
        // Windsurf's language_server without antigravity markers:
        assert!(!is_antigravity_command_line("/x/windsurf/language_server_macos --app_data_dir /x/windsurf"));
    }

    #[test]
    fn extracts_flags_both_syntaxes() {
        assert_eq!(extract_flag("a --csrf_token=tok1 b", "--csrf_token").as_deref(), Some("tok1"));
        assert_eq!(extract_flag("a --csrf_token tok2 b", "--csrf_token").as_deref(), Some("tok2"));
        assert_eq!(extract_flag("a b", "--csrf_token"), None);
    }

    #[test]
    fn parses_lsof_listen_ports() {
        let out = "language_ 423 me 12u IPv4 0x0 0t0 TCP 127.0.0.1:42117 (LISTEN)\n\
                   language_ 423 me 13u IPv4 0x0 0t0 TCP 127.0.0.1:42118 (LISTEN)\n\
                   language_ 423 me 14u IPv4 0x0 0t0 TCP 1.2.3.4:443->5.6.7.8:1 (ESTABLISHED)\n";
        assert_eq!(parse_listening_ports(out), vec![42117, 42118]);
    }

    #[test]
    fn normalizes_quota_summary_buckets() {
        let body = json!({ "response": { "groups": [
            { "buckets": [
                { "bucketId": "3p-weekly", "remainingFraction": 1.0, "resetTime": "2026-07-18T00:00:00Z" },
                { "bucketId": "3p-5h", "remainingFraction": 1.0 },
            ]},
            { "buckets": [
                { "bucketId": "gemini-weekly", "remainingFraction": 0.76 },
                { "bucketId": "gemini-5h", "remainingFraction": 1.0 },
            ]},
        ]}});
        let w = normalize_quota_summary(&body).unwrap();
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Cl 7d", "Cl 5h", "Gm 7d", "Gm 5h"]);
        assert!((w[2].used_percent - 24.0).abs() < 1e-9);
    }

    #[test]
    fn quota_summary_unknown_buckets_is_error() {
        let body = json!({ "response": { "groups": [
            { "buckets": [ { "bucketId": "renamed", "remainingFraction": 0.5 } ] },
        ]}});
        assert!(normalize_quota_summary(&body).is_err());
    }

    #[test]
    fn quota_summary_bad_code_is_error() {
        assert!(normalize_quota_summary(&json!({ "code": 7 })).is_err());
    }

    #[test]
    fn normalizes_user_status_families() {
        let body = json!({ "userStatus": {
            "planStatus": { "planInfo": { "planDisplayName": "Dev" } },
            "cascadeModelConfigData": { "clientModelConfigs": [
                { "label": "Claude Sonnet", "modelOrAlias": { "model": "claude-x" },
                  "quotaInfo": { "remainingFraction": 0.3, "resetTime": "2026-07-18T00:00:00Z" } },
                { "label": "Claude Opus", "modelOrAlias": { "model": "claude-y" },
                  "quotaInfo": { "remainingFraction": 0.9 } },
                { "label": "Gemini Pro", "modelOrAlias": { "model": "gemini-pro" },
                  "quotaInfo": { "remainingFraction": 0.5 } },
                { "label": "Gemini Flash Lite", "modelOrAlias": { "model": "gemini-flash-lite" },
                  "quotaInfo": { "remainingFraction": 0.1 } },
            ]},
        }});
        let (plan, w) = normalize_user_status(&body, false).unwrap();
        assert_eq!(plan.as_deref(), Some("Dev"));
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        // lite model excluded from chat models; claude min=0.3 → Cl 7d 70%,
        // claude max=0.9 → Cl 5h 10%, gemini pro both slots.
        assert_eq!(labels, vec!["Cl 7d", "Cl 5h", "Gm 7d", "Gm 5h"]);
        assert!((w[0].used_percent - 70.0).abs() < 1e-9);
        assert!((w[1].used_percent - 10.0).abs() < 1e-9);
    }

    #[test]
    fn user_status_no_models_is_error() {
        assert!(normalize_user_status(&json!({ "userStatus": {} }), false).is_err());
    }
}
