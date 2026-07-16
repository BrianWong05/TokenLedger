import type { Settings } from '../types';

type Theme = Settings['theme'];

// Apply a theme to the document root by stamping `data-theme` ('light'|'dark'),
// which the CSS-variable palettes in index.css key off. 'system' tracks the OS
// preference live via matchMedia; 'light'/'dark' force. Returns a cleanup that
// detaches the live listener (a no-op for the forced modes).
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): () => void {
  const mql = window.matchMedia?.('(prefers-color-scheme: dark)');
  const resolve = () => (theme === 'system' ? (mql?.matches ? 'dark' : 'light') : theme);
  const set = () => root.setAttribute('data-theme', resolve());
  set();
  if (theme === 'system' && mql) {
    mql.addEventListener('change', set);
    return () => mql.removeEventListener('change', set);
  }
  return () => {};
}
