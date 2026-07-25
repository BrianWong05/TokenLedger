// Which desktop the app is running on. Read once, at the shell's boundary, and
// passed down — nothing else here asks, so there is one answer and one place to
// change it. The webview already knows without a round-trip to Rust: WKWebView
// says Macintosh, WebView2 says Windows, WebKitGTK says Linux.
// ponytail: the user agent rather than @tauri-apps/plugin-os, which would add a
// dependency, a Rust plugin, and a capability entry to learn one word. Reach for
// the plugin if anything ever needs the version or architecture too.
export type Platform = 'macos' | 'windows' | 'linux';

// Whole words, never fragments: "darwin" contains "win". And macOS is what an
// unrecognized agent falls back to, because the two mistakes cost differently —
// missing macOS drops the traffic lights onto the wordmark, while wrongly
// assuming it elsewhere leaves a 30px gap nobody will file a bug about.
export function detectPlatform(ua: string = navigator.userAgent): Platform {
  if (/Windows/i.test(ua)) return 'windows';
  if (/X11|Linux/i.test(ua)) return 'linux';
  return 'macos';
}
