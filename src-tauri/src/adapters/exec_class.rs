// Token-wise port of TokenTracker's exec classifiers (categorizer-utils.js:
// inferExecCommandKind / getExecutableName / sanitizeCommandSignature).
// The regex table is re-expressed over whitespace tokens (trailing ;&|
// trimmed per token); rule ORDER is preserved — the git/http "anywhere"
// rules fire before file_mutation/shell_inspect/compound. The unit tests
// pin equivalence on the rule table's canonical commands.

fn shell_words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if q == '"' && c == '\\' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c.is_whitespace() {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// bash|sh|zsh|fish -lc "<inner>" unwraps to the inner command's words;
// rtk|env|command|xcrun prefixes are stripped recursively.
fn unwrap_shell(words: Vec<String>) -> Vec<String> {
    if words.len() >= 3
        && matches!(words[0].as_str(), "bash" | "sh" | "zsh" | "fish")
        && words[1] == "-lc"
    {
        return shell_words(&words[2..].join(" "));
    }
    if words.len() >= 2 && matches!(words[0].as_str(), "rtk" | "env" | "command" | "xcrun") {
        return unwrap_shell(words[1..].to_vec());
    }
    words
}

fn basename(w: &str) -> String {
    w.rsplit('/').next().unwrap_or(w).to_string()
}

fn is_var_assign(w: &str) -> bool {
    match w.find('=') {
        None => false,
        Some(eq) => {
            let name = &w[..eq];
            !name.is_empty()
                && name.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
                && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }
    }
}

pub fn exec_exe(cmd: &str) -> String {
    let words = unwrap_shell(shell_words(cmd));
    // Skip leading VAR= assignments (e.g. `env FOO=1 git add` -> `git`) to
    // find the real executable. exec_cmd keeps words[0] verbatim by design.
    match words.iter().find(|w| !w.is_empty() && !is_var_assign(w)) {
        Some(w) => basename(w),
        None => "unknown".to_string(),
    }
}

pub fn exec_cmd(cmd: &str) -> String {
    let words = unwrap_shell(shell_words(cmd));
    if words.is_empty() {
        return "unknown".to_string();
    }
    let exe = basename(&words[0]);
    let sub = words
        .iter()
        .skip(1)
        .find(|w| !w.is_empty() && !w.starts_with('-') && !is_var_assign(w));
    match sub {
        Some(s) => format!("{exe} {s}"),
        None => exe,
    }
}

// Trailing shell punctuation glued to a token ("ls;", "grep|") must not
// defeat first-word rules.
fn tok(w: &str) -> &str {
    w.trim_end_matches([';', '&', '|'])
}

pub fn exec_kind(raw: &str) -> &'static str {
    let cmd = raw.trim();
    let words: Vec<String> = shell_words(cmd).iter().map(|w| tok(w).to_string()).collect();
    let w0 = words.first().map(String::as_str).unwrap_or("");
    let w1 = words.get(1).map(String::as_str).unwrap_or("");
    let w2 = words.get(2).map(String::as_str).unwrap_or("");

    // package-manager rules (order matters: build before test etc.)
    if matches!(w0, "npm" | "yarn" | "pnpm") {
        let arg = if w1 == "run" { w2 } else { w1 };
        let is_build =
            arg == "build" || arg.starts_with("build:") || arg.ends_with(":build");
        if is_build {
            return "build";
        }
        if w1 == "test" || (w1 == "run" && w2.contains("test")) {
            return "test";
        }
        if w1 == "run" && w2 == "typecheck" {
            return "typecheck";
        }
        if matches!(w1, "install" | "add" | "ci") {
            return "dependency";
        }
        if matches!(w1, "pack" | "publish" | "version") {
            return "package";
        }
        if w1 == "run" && (matches!(w2, "dev" | "serve" | "start") || w2.contains("dev")) {
            return "dev_server";
        }
    }
    let pair = |a: &str, bs: &[&str]| {
        words.windows(2).any(|w| w[0] == a && bs.contains(&w[1].as_str()))
    };
    if pair("node", &["--check"]) {
        return "syntax_check";
    }
    if w0 == "node" && (w1 == "-e" || (w1 == "--input-type=module" && w2 == "-e")) {
        return "node_eval";
    }
    if w0 == "node"
        && words.iter().any(|w| {
            w.contains("query") || w.contains("analyze") || w.contains("report")
        })
    {
        return "node_cli";
    }
    if w0 == "git" && w1 == "status" {
        return "git_status";
    }
    if pair("git", &["push", "pull", "fetch", "clone"]) {
        return "git_remote";
    }
    if pair("git", &["add", "commit", "branch", "config", "remote", "restore"]) {
        return "git_local";
    }
    if words.iter().any(|w| w == "curl" || w == "wget") {
        return "http";
    }
    if matches!(w0, "ps" | "pgrep" | "pkill" | "kill" | "lsof") {
        return "process";
    }
    if w0 == "tmux" {
        return "terminal";
    }
    if matches!(w0, "open" | "osascript") {
        return "browser_control";
    }
    if matches!(w0, "rm" | "mkdir" | "touch" | "chmod" | "cp" | "mv") {
        return "file_mutation";
    }
    if matches!(w0, "pwd" | "ls" | "test") {
        return "shell_inspect";
    }
    if cmd.contains(';') || cmd.contains('&') || cmd.contains('|') {
        return "compound";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_ported_rule_table() {
        assert_eq!(exec_kind("npm run build"), "build");
        assert_eq!(exec_kind("pnpm run app:build"), "build");
        assert_eq!(exec_kind("npm test"), "test");
        assert_eq!(exec_kind("npm run test:unit"), "test");
        assert_eq!(exec_kind("npm run typecheck"), "typecheck");
        assert_eq!(exec_kind("npm install"), "dependency");
        assert_eq!(exec_kind("npm publish"), "package");
        assert_eq!(exec_kind("npm run dev"), "dev_server");
        assert_eq!(exec_kind("node --check foo.js"), "syntax_check");
        assert_eq!(exec_kind("node -e 'console.log(1)'"), "node_eval");
        assert_eq!(exec_kind("node scripts/report.js"), "node_cli");
        assert_eq!(exec_kind("git status"), "git_status");
        assert_eq!(exec_kind("git push origin main"), "git_remote");
        assert_eq!(exec_kind("git add ."), "git_local");
        // The anywhere-rules fire before file_mutation/compound.
        assert_eq!(exec_kind("rm x && git add ."), "git_local");
        assert_eq!(exec_kind("echo hi | curl -d @- http://x"), "http");
        assert_eq!(exec_kind("ps aux"), "process");
        assert_eq!(exec_kind("tmux ls"), "terminal");
        assert_eq!(exec_kind("open http://x"), "browser_control");
        assert_eq!(exec_kind("rm -rf dist"), "file_mutation");
        assert_eq!(exec_kind("ls; echo done"), "shell_inspect");
        assert_eq!(exec_kind("cd a && npx tsc"), "compound");
        assert_eq!(exec_kind("cd /some/where"), "unknown");
        assert_eq!(exec_kind(""), "unknown");
        assert_eq!(exec_kind("npx vitest"), "unknown");
    }

    #[test]
    fn exe_and_signature_unwrap_and_skip_flags_and_vars() {
        assert_eq!(exec_exe("git add ."), "git");
        assert_eq!(exec_exe("/usr/bin/python3 x.py"), "python3");
        assert_eq!(exec_exe("bash -lc \"git add .\""), "git");
        assert_eq!(exec_exe("env FOO=1 git add"), "git");
        assert_eq!(exec_exe(""), "unknown");
        assert_eq!(exec_cmd("git add ."), "git add");
        assert_eq!(exec_cmd("npx vitest run"), "npx vitest");
        assert_eq!(exec_cmd("cargo test --release e2e"), "cargo test");
        // Faithful port: TZ=UTC is words[0] (not an unwrap prefix), so it IS
        // the "executable"; the signature scanner then finds "npm" as the
        // first non-flag, non-VAR= subcommand. Same for the env variant,
        // which unwraps "env" and leaves TZ=UTC in front.
        assert_eq!(exec_cmd("TZ=UTC npm test"), "TZ=UTC npm");
        assert_eq!(exec_cmd("env TZ=UTC npm test"), "TZ=UTC npm");
        assert_eq!(exec_cmd("sqlite3"), "sqlite3");
        assert_eq!(exec_cmd("grep -rn foo src"), "grep foo");
    }
}
