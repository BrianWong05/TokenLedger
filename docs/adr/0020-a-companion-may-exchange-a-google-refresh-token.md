# A Companion may exchange a Google refresh token

ADR-0019 bound 1 forbids a Companion to spend a refresh token, and Claude's
card lives comfortably inside that bound because Claude Code's access token is
long-lived. Google's are not: Gemini CLI and Antigravity both authenticate
against `cloudcode-pa.googleapis.com` with access tokens that die in about an
hour, so under bound 1 their cards could show a Limit Reading only within an
hour of the person last running the tool — unavailable, for most people, most
of the time (#110, #112).

Bound 1 was written to protect one property: the Companion never corrupts the
Source's own session (tokscale rewrote Claude's credential document and left
Claude Code reporting "Not logged in"). The Google refresh-token grant does
not touch that property. Verified three ways in #110: the grant's response
carries no replacement refresh token; Google's grant-eviction cap counts
authorization grants, which a refresh never creates, so no number of
exchanges can evict the tool's own token; and openusage has run this exact
pattern in the field without burning anyone's session. The literal bound was
broader than the property it protected.

So, for Sources whose credential is a Google OAuth document — today Gemini
CLI (`~/.gemini/oauth_creds.json`) and Antigravity (Keychain
`service=gemini account=antigravity`) — the Companion may present the stored
refresh token to mint an access token, under bounds of its own:

1. It exchanges only against `https://oauth2.googleapis.com/token`, and the
   Source's own credential document is still never written. A stored access
   token that has not expired is used as-is; the exchange happens only on
   expiry.
2. No cache. The minted access token lives only in the Companion's process
   memory — TokenLedger never writes a Google token to disk or Keychain, so
   the property stays greppable rather than promised. The ≥60s floor
   (ADR-0019 bound 3) already bounds exchange frequency. If a cache is ever
   introduced, it must bind to a one-way fingerprint of the current refresh
   credential (openusage binds to SHA-256 of the refresh token) so a logout
   or account switch can never serve the previous account's quota.
3. The OAuth client id/secret pair identifies the *vendor's* app, not ours.
   Google's installed-app model ships both halves in every copy of each
   client — they are public identifiers, not keys. The **id is hardcoded**,
   because the Companion needs a fixed thing to look for; the **secret is
   read at run time out of the vendor's own installed client**, which is
   where it ships and the only copy of it this project ever handles. If the
   client cannot be found, or carries no pair, the exchange does not happen
   and the card degrades per ADR-0019 bound 4: unavailable, pointing at the
   Source's own CLI. The same follows if a vendor rotates its id.

   *Amended when the Antigravity Companion was built.* This bound first read
   "the pairs ride hardcoded in the Companion source", which is safe for the
   reason above but copies a rotating vendor identifier into this repository
   and dates it: a rotation would silently sign every user out until someone
   noticed and edited a constant. Reading the secret from the installed
   client keeps the same posture — we present ourselves as the vendor's app,
   which this ADR already sanctions — while staying current by construction
   and holding no copy of the vendor's identifier at rest. Hardcoding remains
   acceptable where a vendor ships no discoverable client.
4. ADR-0019 bounds 2–4 stand unchanged. Bound 3's person-initiated floor is
   also an obligation to the tool's session, not only a consent rule: the
   refresh grant is rate-limited per client, and the Companion shares each
   tool's client id with the tool itself.
