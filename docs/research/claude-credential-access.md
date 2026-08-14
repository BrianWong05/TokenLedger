# Claude credential access evidence

Status as of 2026-08-12: reading Claude Code's stored OAuth document **does not
prompt on macOS**, and TokenLedger's missing code signature is irrelevant to
that answer — provided the read goes through `/usr/bin/security`. The reason is
not luck. Claude Code creates the Keychain item by shelling out to `security
add-generic-password`, so the item's access-control list trusts
`/usr/bin/security` and nothing else, and any process that runs that same Apple
binary satisfies the ACL. TokenLedger's own signature never enters the
evaluation, which is why "Always Allow" cannot be lost across an unsigned
release — no grant is ever created to lose.

The same evidence carries the risk worth planning for. A *correctly* ACL'd item
does prompt, does re-prompt after an app update, and there are real user
reports to prove it — filed against the Claude *Desktop* item and against a
Tauri app using the `keyring` crate, not against Claude Code. Anthropic has
said publicly that it is "tracking a tightening of the Keychain item's access
control". So the card must treat "could not read the credential" as an ordinary
state, not an error path, and the in-process Keychain API must stay out of the
codebase.

Investigated against Claude Code 2.1.227 on macOS 26.6.1 (arm64), Anthropic's
published documentation, Apple's `security(1)` man page and developer
documentation, and the four prior-art projects with their issue trackers.
Nothing on this machine's Keychain was read and no credential file was opened;
every claim comes from documentation, from strings in Claude Code's own signed
binary, or from prior-art source and issues.

## What Claude Code actually does

The macOS install is a single Developer-ID-signed Mach-O, not a Node script:

```
Executable=~/.local/share/claude/versions/2.1.227
Identifier=com.anthropic.claude-code
Authority=Developer ID Application: Anthropic PBC (Q6L2SF6YDW)
CodeDirectory v=20500 flags=0x10000(runtime)
```

It could therefore have bound the item to its own signature. It does not. The
bundled JavaScript inside that binary reaches the Keychain by spawning Apple's
CLI — the write path is `security add-generic-password -U -a <account> -s
<service> -X <hex>` (with an `-i`/stdin variant, and an argv fallback logged as
`Keychain payload (NB JSON) exceeds security -i stdin limit; using argv`), and
the read path is `security find-generic-password -a <account> -w -s <service>`.
Those three subcommands, plus `delete-generic-password`, are the only Keychain
entry points in the binary. There is no in-process `SecItem*` call, and neither
`-A` nor `-T` is ever passed.

- **Service name.** Built, never a constant, which is why grepping the binary
  for the literal `Claude Code-credentials` finds nothing: `` `Claude
  Code${OAUTH_FILE_SUFFIX}${e}${configDirSuffix}` ``, where `OAUTH_FILE_SUFFIX`
  is `-credentials`, and `configDirSuffix` is `-` plus the first 8 hex of
  `sha256(NFC(configDir))` — empty when neither
  `CLAUDE_SECURESTORAGE_CONFIG_DIR` nor `CLAUDE_CONFIG_DIR` is set. orca
  decompiled the same function out of 2.1.223 in
  [orca#12857](https://github.com/stablyai/orca/issues/12857) and openusage
  reproduces it in `keychainServiceCandidates()`. The suffix carries
  `-staging-oauth` / `-local-oauth` / `-custom-oauth` for Anthropic-internal
  and custom-endpoint logins, empty in production.
- **But the derivation has changed across versions, so the name must be probed,
  not computed.** openusage
  [#423](https://github.com/robinebers/openusage/issues/423) is a real machine
  with no `.credentials.json`, a dead legacy item, and the live credential
  under `Claude Code-credentials-<8 hex>` where the suffix matched
  `sha256("/Users/<user>/.claude").slice(0,8)` — i.e. a hash of the *default*
  config dir, with no `CLAUDE_CONFIG_DIR` set at all, which the 2.1.227 logic
  above would not produce. It cross-references anthropics/claude-code #19456
  and #20553. Try several names.
- **Account name.** `process.env.USER`, then `os.userInfo().username`, then the
  literal `claude-code-user` — and anything failing `/^[a-zA-Z0-9._-]+$/` is
  replaced rather than passed through. This is a live trap, not a curiosity:
  [orca#12857](https://github.com/stablyai/orca/issues/12857) is a machine
  whose `$USER` is `first@example.com`, where Claude Code stored the item under
  `claude-code-user` and every one of orca's `-a "$USER"` lookups missed.
- **Failure classification.** Claude Code parses `security`'s stderr into
  `interaction_not_allowed` (`"interaction is not allowed"`, `"no user
  interaction"`), `user_canceled` (`errSecUserCanceled`, `"cancel"`),
  `auth_failed` (`errSecAuthFailed`, `"authorization"`, `"authentication"`,
  `"name or passphrase"`), `keychain_locked` (`"locked"`, `"unlock"`) and
  `other`. Anthropic would not have written that ladder if these were
  theoretical; it is the real failure surface of this API and a consumer wants
  the same taxonomy.
- **The document is cached, not re-read per call** — a 30 s cache and a 1 s
  failure backoff in front of the read, plus an mtime watcher on
  `.credentials.json`. Decision 5's 60 s floor is already stricter.

Anthropic's documentation states the storage locations but never the item name:
"On macOS, credentials are stored in the encrypted macOS Keychain"; "On Linux,
credentials are stored in `~/.claude/.credentials.json` with file mode `0600`";
"On Windows, credentials are stored in
`%USERPROFILE%\.claude\.credentials.json` and inherit the access controls of
your user profile directory, which restricts the file to your user account by
default" ([Authentication → Credential
management](https://code.claude.com/docs/en/authentication#credential-management)).
The Security page hedges once — tokens are "stored in the macOS Keychain **when
available**" ([Security](https://code.claude.com/docs/en/security)) — without
saying what happens when it is not, and `.credentials.json` is not listed in
the [`~/.claude` directory
reference](https://code.claude.com/docs/en/claude-directory) at all.

## Why there is no prompt

Apple's `security(1)` man page, on `add-generic-password`:

> `-A`  Allow any application to access this item without warning (insecure,
> not recommended!)
> `-T appPath`  Specify an application which may access this item (multiple
> -T options are allowed)
>
> By default, the application which creates an item is trusted to access its
> data without warning. You can remove this default access by explicitly
> specifying an empty app pathname: `-T ""`.

Claude Code passes neither flag, so the default applies — and "the application
which creates an item" is the process that called the Keychain API, which is
`/usr/bin/security` itself, not Claude Code. The ACL's sole trusted application
is a general-purpose Apple tool any process may invoke.

Apple's own engineer demonstrated exactly this, in exactly this shape. In
[Developer Forums thread
116579](https://developer.apple.com/forums/thread/116579), Quinn (DTS) adds an
item with `security add-generic-password`, reads it back with `security
find-generic-password -w`, gets the password with no dialog, and explains why:
"The item was created by the `security` tool, so its ACL is set to allow
unfettered access by that tool." The original poster supplies the same fact
from the other side — the item "has to be created with `-T \"\"` so that
`security` is not added to the list of applications which are always allowed to
access the item without warning" — and with `-T ""` the read *does* prompt.
Claude Code creates its item the first way.

The general rule behind it is Apple's [Access Control
Lists](https://developer.apple.com/documentation/security/keychain_services/access_control_lists)
documentation: the system "checks whether **the calling app** is among the
entry's trusted apps. If so, the system grants access. Otherwise, the system
prompts the user for confirmation" — Deny / Allow / Always Allow, the last of
which "adds the app to the list of trusted apps for that entry, enabling the
app to gain access in the future without prompting the user again". For the CLI
route the calling app is `/usr/bin/security`, and it is already on the list.

Published security research says the same thing and got Anthropic to confirm
it. Silverfort, 2026-07-28, ["Skipping the lock: a Claude Code CLI weakness
lets any macOS process read stored
credentials"](https://www.silverfort.com/blog/skipping-the-lock-a-claude-code-cli-weakness-lets-any-macos-process-read-stored-credentials/):
"an ACL whose only trusted reader is the `security` tool is satisfied by any
process that runs the `security` tool, which means every process on the
machine", with no password or biometric prompt, and the remedy being to use the
Keychain Services API so the item binds to the Claude binary's code signature —
"the approach already implemented correctly in Claude Desktop". Anthropic
acknowledged the report without disputing any fact, called it a design flaw
rather than a vulnerability, and said it is "tracking a tightening of the
Keychain item's access control as a hardening improvement". No CVE.

Three consequences follow, and they are the answer to this ticket.

**TokenLedger's signature is not part of the evaluation.** ACL trust is keyed
to the *calling* process's code identity, and the caller here is
`/usr/bin/security` — Apple's, stable across every macOS update. Unsigned,
ad-hoc signed, or re-signed with a different hash per release changes nothing,
because TokenLedger is never the Keychain client.

**The prompt would be real for the in-process route.** A direct
`SecItemCopyMatching` with `kSecReturnData` — or the `keyring` crate, which is
the same thing with a nicer API — from TokenLedger's own process is where the
unsigned-build problem bites, and this is not hypothetical: a Tauri developer
reported precisely it in
[keyring-rs#272](https://github.com/open-source-cooperative/keyring-rs/issues/272),
"Every time I rebuild my app, I get the question from keychain if I really want
to allow the app to read the information stored in the keychain… **But, I see
the same issue even when the entry is created by the production app.**" The
crate maintainer confirmed the rule rather than offering a fix: "Every time an
app reads a legacy keychain entry written by another app (where app identity
changes with each build), you will see this dialog", adding that the settings
which would allow sharing "aren't available via `keyring`". Tauri's own tracker
carries the same complaint from the development side —
[tauri#7930](https://github.com/tauri-apps/tauri/issues/7930): "every time you
run `tauri dev`, macOS will think it is a new unauthorized app, so you will
have to enter the password 100 times in this dialog" — and
[tauri#8662](https://github.com/tauri-apps/tauri/issues/8662) is a duplicate
symptom from WebKit's own secure storage.

Apple is explicit about why. From [TN3127: Inside Code Signing:
Requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements):
"Unsigned code has no DR. Ad hoc signed code, called Sign to Run Locally by
Xcode, has a DR but it's tied to that specific version of the code. In both
cases macOS can't reliably track the identity of the code… If you tweak the
code and run it again, macOS repeats that prompt." For properly signed code the
ACL records the designated requirement instead, which is what makes grants
survive updates — Quinn again, [thread
115425](https://developer.apple.com/forums/thread/115425): "the keychain
doesn't store, say, a checksum of the app, meaning that version 1.1 of the app
is treated just like version 1.0", with the corollary "if your code is
incorrectly signed, or you change the app on disk in a way that invalidates the
code signature, nothing is going to work reliably". Apple even documents the
re-prompt to end users: "If an app is changed or infected by a virus after
being granted access to your keychain, the app is no longer trusted, and you
must grant access again", and the dialog returns "if you recently updated your
system software or the app, **or if the app has been modified**" ([Keychain
Access
help](https://support.apple.com/guide/keychain-access/if-a-trusted-app-asks-for-keychain-access-kyca1331/mac)).

And TokenLedger's builds *are* modified every time, in the sense macOS cares
about. `tauri build` with no `signingIdentity` configured does not sign at all
— `keychain()` in `crates/tauri-bundler/src/bundle/macos/sign.rs` returns
`None` and the whole sign-and-notarize block is skipped, silently, with
`bundle.macOS. entitlements` and `hardenedRuntime` becoming no-ops because both
are only ever passed to `codesign`. But the binary is not therefore unsigned:
on Apple Silicon the linker ad-hoc signs it, because it must. Quinn, [thread
678816](https://developer.apple.com/forums/thread/678816): "Unsigned code won't
even *run* on Apple silicon Macs… You can sign your code ad-hoc… by adding the
`-adhoc_codesign` linker flag. **This is the default for Apple silicon
builds.**" A Tauri maintainer ran `codesign -dv` on exactly such a build in
[tauri#8763](https://github.com/tauri-apps/tauri/issues/8763) and got
`flags=0x20002(adhoc,linker-signed)`, `TeamIdentifier=not set`. Per
`codesign(1)`, ad-hoc signing "does not use an identity at all, and identifies
exactly one instance of code", and linker signatures "will usually not contain
any embedded code requirements including a designated requirement". So each
release carries a fresh per-build cdhash and no stable DR — the exact condition
TN3127 says macOS cannot track across versions.

So the ticket's fear is entirely sound. It applies to a route we are not
taking.

**All four prior-art projects avoid it, including the signed one.** That
convergence is worth more than any single document.

| Project | How it reads the secret | Notable |
|---|---|---|
| [orca](https://github.com/stablyai/orca) | `security find-generic-password -s <svc> -a $USER -w`, 3 s timeout | Also *writes*; kills the child itself because Node's `execFile` timeout only signals the process. The `-a` is what #12857 broke on |
| [openusage](https://github.com/robinebers/openusage) | `/usr/bin/security find-generic-password -a <user> -w -s <svc>`, then again without `-a`, 5 s timeout | A **signed** Swift app that still shells out |
| [tokscale](https://github.com/junhoyeo/tokscale) | `Command::new("security").args(["find-generic-password","-s",svc,"-w"])` | Service-only, so immune to the `$USER` trap |
| [TokenTracker](https://github.com/xiufengsun/TokenTracker) | `spawnSync("/usr/bin/security", ["find-generic-password","-s",svc,"-w"])`, 2 s timeout | Service-only; separate probe without `-w`; **opt-in, default off** |

openusage is the informative one. It is a signed app that *could* have called
the framework in-process, and does so exactly once — an attributes-only
existence probe on the launch path, deliberately declawed with
`kSecUseAuthenticationUI: kSecUseAuthenticationUIFail`, which the comment says
"never requests the secret and forbids any UI, so it can neither trigger an
unlock prompt nor stall launch", reporting `nil` ("unknown") rather than a
definite answer when the probe fails. Every read that returns a secret goes
through `/usr/bin/security` (`Sources/OpenUsage/Services/SystemClients.swift`).
A team that shipped a signed, notarized app reached for the subprocess anyway.

## The prompt is real for a differently-created item

This is the boundary of the finding, and it deserves its own section because it
is what would break if any premise moved.

openusage's *other* Claude source is Claude Desktop's Electron `safeStorage`
key — service `Claude Safe Storage`, account `Claude Key` — and it reads that
one in-process with `SecItemCopyMatching` and `kSecReturnData: true`
(`ClaudeDesktopAuthStore.swift`). That item is ACL'd to its creator. The
results are on the record:

- [openusage#1071](https://github.com/robinebers/openusage/issues/1071) — a
  user posts a screenshot of the macOS password dialog and asks "Is it safe to
  accept this?". The maintainer's close is the cleanest statement anywhere of
  the distinction this ticket is about: "This is a requirement to read the
  Claude Desktop app credentials. So yes, accepting this is a must for the
  desktop app (**not required for the CLI**)."
- The same thread, a different user: "Why is this requested now? It was working
  before. And how can I disable this request for good? Denying just works
  temporary and **I get asked each update again every 5 minutes**." That is the
  exact failure the ticket feared — "Always Allow" not surviving an update,
  then re-prompting on the refresh cadence — observed on a *signed* app.
- openusage's own security review,
  [#1065](https://github.com/robinebers/openusage/issues/1065), predicts it for
  the Claude Code item too if the route changes: "In-process writes make
  OpenUsage the accessing app rather than `/usr/bin/security`. For keychain
  items another tool created (Claude Code, the `codex` CLI), macOS may show a
  one-time ACL prompt on the first token rotation after updating."
- openusage's dev docs name the ad-hoc mechanism directly
  (`docs/debugging.md`): "The script signs with a stable Apple Development
  identity so the permission ACLs stick. If you see repeated prompts, make sure
  such an identity exists in your keychain (the script warns when it falls back
  to ad-hoc signing)."

And shelling out to `security` is **not** a universal escape — it works here
only because Claude Code's item trusts `security`.
[TokenTracker#369](https://github.com/xiufengsun/TokenTracker/issues/369) is
the counter-case, against an item created properly by a browser: "Even after
entering the user password and selecting **'Always Allow'** … the system
authorization fails to persist. As a result, the security prompt keeps popping
up endlessly during background sync/polling intervals", for `security wants to
use your confidential information stored in "Chrome Safe Storage"`. The fix in
v0.84.8 was to stop calling `security` for that path entirely. Keychain
partitioning is the likely reason Always Allow could not stick — a mechanism
Quinn describes as "a bit of a dark art because they were added to the
file-based keychain long after it was initially introduced. Thus, they have no
APIs and the docs are kinda minimal" ([thread
756171](https://developer.apple.com/forums/thread/756171)) — and it is a
second, independent check that a third-party app calling `SecItem` directly
would also have to pass ("A program from team A can't access keychain items
created by team B, or the system, without a security alert", [thread
696956](https://developer.apple.com/forums/thread/696956)).

Worth recording for honesty: Apple advises against the route recommended here.
"I recommend against running the `security` tool for this task. It's definitely
going to cause more problems than it solves" ([thread
712902](https://developer.apple.com/forums/thread/712902)) and "the `security`
tool is not considered API and I strongly recommend against you using it as
such" ([thread 671582](https://developer.apple.com/forums/thread/671582)). The
advice is sound in general and there is no supported alternative for reading
another application's item — Quinn in the same breath: "there's no supported
way for you to create keychain items which will be silently used by other apps
(or the system). User consent must be acquired" ([thread
98182](https://developer.apple.com/forums/thread/98182)). We are reading a
credential Anthropic left world-readable by accident. The route is unsupported
by Apple and contingent on Anthropic's ACL choice; both facts belong in the
risk register rather than in a footnote.

## Prompt behaviour per platform

**macOS — no prompt, no dialog, no "Always Allow" to lose,** for the `security`
CLI route. Not once, not per fetch. The failures that remain are unrelated to
ACLs: a locked login Keychain (Anthropic documents this as a login failure
cause, with `claude doctor` as the check —
[Troubleshooting](https://code.claude.com/docs/en/troubleshoot-install#login-and-authentication)),
a Keychain password out of sync with the account password, and the `security`
child hanging — all four projects impose a 2–5 s timeout, and Claude Code's own
2.1.225 changelog entry fixes "MCP OAuth servers on macOS intermittently
failing with a burst of 401 errors … after a keychain read timed out". Note too
that a GUI app launched from Finder inherits no login shell, so `USER` may be
absent in a Tauri process.

**Linux — no prompt.** `~/.claude/.credentials.json`, mode `0600`, owned by the
user, relocated by `CLAUDE_CONFIG_DIR`. TokenLedger runs as the same user: an
ordinary read, no elevation, no dialog, no libsecret or Secret Service to
negotiate.

**Windows — no prompt.** `%USERPROFILE%\.claude\.credentials.json`, protected
only by the inherited profile ACL — no DPAPI, no Credential Manager. Same-user
read, no elevation, no UAC. (No prior-art project uses DPAPI for this; orca's
only DPAPI use is unrelated browser-cookie import.)

The Keychain is the only platform where the question arises, and there the
answer is no.

## Is there a file fallback on macOS?

Partly, and not dependably. Claude Code's binary contains an unconditional
`readFile(join(CLAUDE_CONFIG_DIR ?? ~/.claude, ".credentials.json"))` on its
credential-resolution path plus an mtime watcher on the same file, so the file
is read on macOS when it exists. On this machine — Claude Code 2.1.227, macOS
26.6.1, an active logged-in install — **it does not exist**. Anthropic scopes
the `CLAUDE_CONFIG_DIR` relocation sentence to "Linux or Windows", never
documents a macOS file path, and prescribes fixing the Keychain rather than
falling back to a file. Community reports go both ways: SSH and Remote-SSH
users report re-login loops *despite* a valid `.credentials.json`
([#29816](https://github.com/anthropics/claude-code/issues/29816),
[#44089](https://github.com/anthropics/claude-code/issues/44089)), and one
reports a macOS install deleting the file a Linux install was using
([#10039](https://github.com/anthropics/claude-code/issues/10039)).

Treat the file on macOS as a possible leftover, never as the source of truth.
openusage states the rule outright: the keychain is "Claude Code's source of
truth on macOS — recent versions keep the current session there and can leave a
stale `~/.claude/.credentials.json` behind — so it must win when valid". It
learned this twice: ranking candidates purely by token expiry let a stale file
outrank a live keychain (their #738), while never falling through at all missed
an external `claude` re-login that landed in the other store (#687). tokscale
reads the file *first*, which is the ordering openusage regressed on;
TokenTracker reads the keychain on macOS and the file on Linux/Windows with no
cross-fallback either way.

## Why orca has a PTY fallback

The ticket's hint was that orca screen-scraping `/usage` through a PTY implies
direct reads fail somewhere real. They do — but not because of prompts. Nothing
in orca's code, comments, tests, or issues mentions a Keychain prompt. Its
`claude-pty.ts` is reached from `claude-fetcher.ts` as a catch-all for:

- **any `security` exit that is not 44** — locked keychain, access denied; its
  test mocks `new Error('Keychain locked')` explicitly;
- **`security` hanging**, capped at 3 s, with the comment that "Node's
  `execFile` timeout only signals the `security` process; a stuck callback
  would otherwise leave auth/keychain operations pending". A blocking modal is
  exactly what that guard catches, but orca never says so — inference, not
  evidence;
- **credentials it holds but must not spend** — refresh-only documents, and
  `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`, where the comment reads "those
  are API keys that 401 on the OAuth usage endpoint (PTY fallback serves
  them)";
- **the item having moved**, per the 2.1+ config-dir scoping;
- **platforms with no Keychain at all**, including a full `wsl.exe -d <distro>
  -- bash -lc '… exec claude'` branch.

No git provenance is available for this: all four clones are `--depth 1`, so
`git log -S`, `--follow`, and `blame` return nothing. The reconstruction above
is from code, comments, tests, and the GitHub API.

## Recommended acquisition route

Read-only, per map decision 3, and in this order:

1. **macOS: `/usr/bin/security find-generic-password -s <service> -w`**, as a
   subprocess with a ~3 s timeout, an absolute path, and output that is never
   logged. **Do not pass `-a`.** orca's `-a "$USER"` is the one variant with a
   filed bug against it, and orca's own recommendation after decompiling Claude
   Code is service-only resolution, which "stays correct if Anthropic changes
   the derivation again". Try these service names in order and take the first
   hit: `Claude Code-credentials`; then `Claude Code-credentials-<first 8 hex
   of sha256(NFC(dir))>` for `dir = $CLAUDE_SECURESTORAGE_CONFIG_DIR`,
   `$CLAUDE_CONFIG_DIR`, and the expanded default `~/.claude` — #423 proves the
   default-dir hash occurs in the wild. Exit 44 is `errSecItemNotFound`, the
   honest "not signed in"; **any other non-zero exit is a failure, not an
   absence** and must not render as "not signed in". Classify stderr with
   Claude Code's own taxonomy so a locked Keychain says it is locked.
2. **All platforms: `$CLAUDE_CONFIG_DIR/.credentials.json`, else
   `~/.claude/.credentials.json`.** The sole source on Linux and Windows; on
   macOS a fallback that must lose to a valid Keychain read.
3. **Do not use `CLAUDE_CODE_OAUTH_TOKEN` for the gauge.** It is official and
   documented, but `claude setup-token` mints an inference-only one-year token
   — "It can only make model requests"
   ([Authentication](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token))
   — and openusage confirms from the field that it "403s on the usage
   endpoint", keeping it as a trailing `inferenceOnly` candidate after two
   issues about it (#901, #782). If it is exported ambiently it must not shadow
   a real login.
4. **Never write, never refresh, never spend the refresh token.** Already
   decision 3; the field evidence is now overwhelming. tokscale's
   [#1001](https://github.com/junhoyeo/tokscale/issues/1001) rebuilt
   `.credentials.json` from four fields, destroying `expiresAt` and `scopes`,
   and logged Claude Code out ten seconds after starting — and the maintainer
   found two further defects while fixing it, including that it "can source
   from the macOS Keychain, but `save_credentials()` unconditionally wrote the
   file", *creating* a partial file on a keychain-only machine. Their fix
   deleted the refresh path, the client ID, and even the `refreshToken` field,
   with a test asserting exactly one HTTP request is made. orca's
   [#9582](https://github.com/stablyai/orca/issues/9582) is worse: an app
   self-update raced and clobbered Claude Code's credentials, and replaying the
   dead token family tripped "Anthropic's token-theft detection", which
   "revokes the whole tree — killing the live session too". Not modelling
   `refreshToken` at all is the cheapest guarantee here — tokscale's reason is
   worth quoting: "tokscale has no use for a credential it must not spend."
5. **Check `scopes` before fetching.** The usage endpoint needs `user:profile`;
   openusage treats an absent or empty list as "unknown, allow" (older
   credentials predate the field) and a present list lacking the scope as a
   "sign in again" state rather than blank bars.
6. **For card state, probe without reading the secret.** `security
   find-generic-password -s <service>` with no `-w` returns exit 0/44 and no
   secret — TokenTracker does exactly this, calling it an "existence-only
   probe: do not read secrets" — and an in-process `SecItemCopyMatching` with
   `kSecUseAuthenticationUIFail` and no `kSecReturnData` answers the same
   question in microseconds with UI forbidden. Either satisfies decision 8's
   "no credential is read until the button is pressed" while still letting the
   card say whether Claude is signed in. TokenTracker's keychain probing is
   opt-in and default off, which is the same posture.

Three things not to do, each of which looks like the obvious move:

- **Do not reach for the `keyring` crate.** Its macOS backend is the legacy
  `SecKeychain` API against the login keychain — `find_generic_password` from
  `security-framework`, deprecated by Apple — which is precisely the API family
  governed by per-item ACLs and the Allow / Always Allow dialog. That is
  keyring-rs#272's bug, and the maintainer's own advice there is to stop using
  the crate for this. It is also more code than `Command::new("security")`.
- **Do not try to suppress the dialog.**
  `SecKeychainSetUserInteractionAllowed(false)` and
  `kSecUseAuthenticationUISkip` do not grant access, they convert the prompt
  into `errSecInteractionNotAllowed` (-25308) or `errSecInteractionRequired`
  (-25315). Useful for a silent probe, useless for a read.
- **Do not "fix" this by setting `"signingIdentity": "-"`.** Ad-hoc signing
  gives no stable identity — it would not help even if we needed one — and
  tauri#8763 documents it breaking bundled binaries outright, where the ad-hoc
  main executable and ad-hoc dylibs get different Team IDs and dyld refuses to
  load them. TokenLedger ships a sidecar; that is not a knob to turn casually.

A route worth knowing about *instead of* the credential read, if the endpoint's
unofficial status becomes uncomfortable: Claude Code's status line receives
`rate_limits.five_hour.used_percentage`,
`rate_limits.seven_day.used_percentage` and `rate_limits.*.resets_at` on stdin
as **documented** fields ([Status
line](https://code.claude.com/docs/en/statusline)), present "only for Claude.ai
subscribers (Pro/Max) after the first API response in the session". A
`statusLine` command that appended those to a file would give TokenLedger the
same three numbers with no credential, no Keychain, and no undocumented HTTP —
at the cost of writing to the user's Claude Code settings, colliding with any
status line they already run, and only updating while Claude Code is live. Not
a v1 recommendation; a real hedge if `/api/oauth/usage` is withdrawn.

## Disabled-card copy

Per decision 9 a credential-less tool still gets a card. These states are
distinguishable and should not collapse into one message — openusage's warning
that a non-44 exit "must not be silently rendered as 'not signed in'" is a bug
it has not finished fixing, and TokenLedger can start on the right side of it:

- **Not signed in** (exit 44, or no file): *"Not signed in. Run `claude` in a
  terminal to log in."*
- **Keychain unreadable** (any other non-zero exit): *"macOS couldn't read
  Claude's saved login — the login keychain may be locked. Unlock it and
  refresh."* Never show this as "not signed in"; that sends the user to
  re-authenticate a login they already have.
- **Signed in, but this token can't read limits** (no `user:profile`, or only
  `CLAUDE_CODE_OAUTH_TOKEN`): *"This Claude login can't read usage limits. Run
  `claude` and sign in again to enable them."*
- **Rejected on fetch** (401/403): *"Claude rejected the saved login. Run
  `claude` to sign in again."* Nothing is written or refreshed in response.
- **Not available here** (Linux/Windows, no credentials file): same copy as
  "not signed in" — the mechanism differs, the user's action does not.

openusage's strings set the register to match: "Not logged in. Run `claude` to
authenticate.", "Session expired. Run `claude` to log in again."

## Verdict on the unsigned-build problem

**Not real, for the recommended route.** The premise — ACL trust keyed to
TokenLedger's signature, an ad-hoc signature that changes per build, therefore
a prompt on every release — is correct in general and simply does not apply,
because TokenLedger is never the Keychain client. Confidence is high. It rests
on Apple's man page for the default ACL, on a DTS engineer demonstrating the
create-with-`security`/read-with-`security` case and explaining it in those
terms, on Claude Code's own binary showing which API creates the item, on
published research stating that any process can read it silently, on
Anthropic's acknowledgement of that research, and on four independent projects
— one signed and notarized — having converged on the subprocess route. The
feared failure would appear in those trackers as prompt reports against the
Claude Code item, and it does not appear: the one prompt report is against
Claude Desktop's item, and the maintainer answered it by saying the CLI needs
no such approval.

The premise *would* apply if TokenLedger called `SecItemCopyMatching`
in-process. That is the one implementation choice the spec should forbid
outright, and the reason to forbid it is now documented rather than suspected.

What is genuinely at risk is upstream. Anthropic is on record intending to
tighten this ACL. When that lands, the Claude Code item behaves like the Claude
Desktop item — a prompt, an "Always Allow" that *is* keyed to our signature,
and therefore a real re-prompt-per-release problem for an unsigned build,
exactly as openusage#1071's second reporter experienced. That is the
contingency the disabled card exists for, and one more reason "couldn't read
the credential" must be a first-class card state rather than an error path.

## Open questions

- **Not verified live**, by instruction. One command settles the central claim
  on this machine: `security find-generic-password -s "Claude
  Code-credentials"` *without* `-w`, which prints attributes and no secret.
  Exit 0 with no dialog confirms both the service name and the absence of a
  prompt. A human should run it before the card is built. Adding `-w` in a
  throwaway shell would confirm the secret path too, but that prints the token
  and is not worth it.
- **The ACL's actual contents** are Silverfort's claim plus the man page's
  documented default, not a dump. `security dump-keychain -a` would show the
  trusted-application list but prompts per item; if certainty is wanted,
  inspect a *test* item created the same way rather than the real one. The
  item's `SecKeychainPromptSelector` flags — one of which forces a passphrase
  on every access — are likewise unverified.
- **Whether keychain partitioning applies here.** It is a second check beyond
  the ACL, undocumented by Apple, and it is the likely reason
  TokenTracker#369's "Always Allow" would not persist. For an item created by
  `security` it should be satisfied by `security`, but this was not confirmed.
- **Whether a signed TokenLedger's grant would survive its own updates**, if
  Anthropic's hardening lands and signing becomes the answer. Apple says
  DR-keyed trust persists across updates for correctly signed code, and the fix
  would be a real Developer ID identity in `bundle.macOS.signingIdentity`
  rather than `-`. What no primary source states outright is that
  legacy-keychain ACL evaluation uses the DR mechanism *specifically* — Apple's
  own examples are TCC. Strong inference, not a quote. Nor did anyone print
  `codesign -d -r-` for a linker-signed Tauri binary to confirm it carries no
  DR at all; two consecutive builds would settle it.
- **Claude Desktop's item** is a second possible source and is properly ACL'd,
  so it prompts and needs "Always Allow" UX. Out of scope for v1; worth knowing
  before anyone proposes it as the fallback.
- **Multiple accounts.** The service name is scoped by config dir, not by
  account, so two logins under one `CLAUDE_CONFIG_DIR` cannot both be stored.
  Service-only lookup would silently take the first item if a machine somehow
  holds several. Feeds the map's open "multiple accounts per tool" question.
- **The endpoint is unofficial.** `GET /api/oauth/usage` and `anthropic-beta:
  oauth-2025-04-20` appear in no Anthropic documentation; the official
  rate-limit APIs are org-scoped and need an Admin key, and one report has the
  API rejecting that beta header outright
  ([#13770](https://github.com/anthropics/claude-code/issues/13770)). The only
  documented acknowledgement of the endpoint's existence is `/usage`'s failure
  text about "the usage endpoint" being rate limited
  ([Costs](https://code.claude.com/docs/en/costs#using-the-usage-command)).
  TokenTracker's comment that "Claude shares its OAuth usage endpoint budget
  with Claude Code itself" is worth heeding against decision 5's floor. Not
  this ticket's question, but it bounds what the card can promise.
- **`~/.claude/policy-limits.json`** exists on this machine (0600, written
  2026-08-12) and was not opened. The name suggests plan policy rather than
  live utilization, but the map's "Claude Code persists no live limit state to
  disk" fact predates it. Worth one look by whoever owns the data-source
  ticket.
