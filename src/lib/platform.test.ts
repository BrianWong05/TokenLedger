import { describe, expect, it } from 'vitest';
import { detectPlatform } from './platform';

// The real agents the three webviews send. WKWebView and WebView2 are stable
// strings; WebKitGTK still says X11 under Wayland.
const WKWEBVIEW =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15';
const WEBVIEW2 =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36';
const WEBKITGTK =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15';

describe('detectPlatform', () => {
  it('reads each webview by the agent it actually sends', () => {
    expect(detectPlatform(WKWEBVIEW)).toBe('macos');
    expect(detectPlatform(WEBVIEW2)).toBe('windows');
    expect(detectPlatform(WEBKITGTK)).toBe('linux');
  });

  // "darwin" contains "win", so a substring match reads a Mac as a PC. jsdom
  // sends exactly that, which is what made this worth pinning.
  it('does not mistake darwin for Windows', () => {
    expect(detectPlatform('Mozilla/5.0 (darwin) AppleWebKit/537.36 jsdom/29.1.1')).toBe('macos');
  });

  it('falls back to macOS, the platform that breaks if we guess wrong', () => {
    expect(detectPlatform('something nobody has shipped yet')).toBe('macos');
  });
});
