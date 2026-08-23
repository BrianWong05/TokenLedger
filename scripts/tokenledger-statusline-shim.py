#!/usr/bin/env python3
"""TokenLedger statusline shim.

Claude Code invokes the statusLine command on every render with a JSON status
document on stdin that includes `rate_limits` — the session's own belief about
the vendor's Limit windows. This shim taps that push channel (no credential,
no fetch, nothing to 429):

  1. extracts `rate_limits` and rename-writes it as TokenLedger's claude
     Export Artifact (schema 4), only when it says something new and never
     regressing a newer artifact,
  2. pipes the same stdin through to ccstatusline unchanged, so the
     statusline renders exactly as before.

Install: copy anywhere stable, then point ~/.claude/settings.json at it:
    "statusLine": { "type": "command", "command": "python3 /path/to/tokenledger-statusline-shim.py" }

The artifact fields mirror what the claude-limits Companion writes so the two
writers land in one Series: same metering regime, same evidence on the two
named windows, plan/account read from ~/.claude.json's non-credential fields.
The stamp is receipt time — Claude Code's belief may itself be minutes old,
which is the bounded dishonesty accepted when this channel was chosen.

Debug: the first render's full stdin payload is kept at
~/.claude/tokenledger-shim-payload.json (delete it to re-capture).
"""
import json
import os
import subprocess
import sys
import tempfile
import time

HOME = os.path.expanduser("~")
LIMITS_DIR = os.environ.get(
    "TOKENLEDGER_LIMITS_DIR",
    os.path.join(HOME, "Library", "Application Support", "com.brianwong.tokenledger", "limits"),
)
ARTIFACT = os.path.join(LIMITS_DIR, "claude.tokenledger-limits.json")
PAYLOAD_CAPTURE = os.path.join(HOME, ".claude", "tokenledger-shim-payload.json")

# The Companion's own evidence mapping for the named windows; per-model keys
# carry none there either, so none here.
EVIDENCE = {
    "five_hour": {"limit_id": "session", "model_scope": "all"},
    "seven_day": {"limit_id": "weekly_all", "model_scope": "all"},
}
WINDOW_MINUTES = {"five_hour": 300}


def _nonblank(value):
    if isinstance(value, str) and value.strip():
        return value
    return None


def identity():
    """plan + account from ~/.claude.json's non-credential fields, best-effort."""
    try:
        with open(os.path.join(HOME, ".claude.json")) as f:
            doc = json.load(f)
    except Exception:
        return None, None
    oauth = doc.get("oauthAccount") or {}
    plan = _nonblank(oauth.get("userRateLimitTier")) or _nonblank(oauth.get("subscriptionType"))
    account = _nonblank(oauth.get("accountUuid")) or _nonblank(
        (doc.get("cachedUsageUtilization") or {}).get("accountUuid")
    )
    return plan, account


def windows(rate_limits):
    out = []
    for key in sorted(rate_limits):
        bucket = rate_limits[key]
        if not isinstance(bucket, dict):
            continue
        used = bucket.get("used_percentage")
        resets = bucket.get("resets_at")
        if not isinstance(used, (int, float)) or not isinstance(resets, (int, float)):
            continue  # a window with no reset instant proves nothing
        win = {
            "key": key,
            "used_pct": float(used),
            "resets_at": int(resets),
        }
        minutes = WINDOW_MINUTES.get(key, 10080 if key.startswith("seven_day") else None)
        if minutes is not None:
            win["window_minutes"] = minutes
        if key in EVIDENCE:
            win["evidence"] = EVIDENCE[key]
        out.append(win)
    return out


def write_artifact(document):
    rate_limits = document.get("rate_limits")
    if not isinstance(rate_limits, dict):
        return
    wins = windows(rate_limits)
    if not wins:
        return
    # Unchanged windows are the same observation — no churn, and a newer
    # artifact (a live Companion fetch moments ago) is never regressed.
    now = int(time.time())
    try:
        with open(ARTIFACT) as f:
            held = json.load(f)
        if held.get("windows") == wins or held.get("fetched_at", 0) >= now:
            return
    except Exception:
        pass
    plan, account = identity()
    export = {
        "schema": 4,
        "source": "claude",
        "fetched_at": now,
        "plan": plan,
        "metering_regime": "claude:usage_limits",
        "windows": wins,
    }
    if account:
        export["account_id"] = account
    os.makedirs(LIMITS_DIR, exist_ok=True)
    fd, staging = tempfile.mkstemp(dir=LIMITS_DIR, suffix=".json.part")
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(export, f)
        os.replace(staging, ARTIFACT)
    except Exception:
        try:
            os.unlink(staging)
        except OSError:
            pass
        raise


def main():
    raw = sys.stdin.buffer.read()
    # The statusline must render whatever the tap does — every failure here
    # is swallowed so a shim bug can never blank the statusline.
    try:
        if not os.path.exists(PAYLOAD_CAPTURE):
            with open(PAYLOAD_CAPTURE, "wb") as f:
                f.write(raw)
        write_artifact(json.loads(raw.decode("utf-8", "replace")))
    except Exception:
        pass
    proc = subprocess.run(["bunx", "-y", "ccstatusline@latest"], input=raw)
    sys.exit(proc.returncode)


if __name__ == "__main__":
    main()
