// Brand icons for each source, imported as asset URLs (Vite resolves each
// import to a URL string; SVGs small enough get inlined as data:image/svg+xml
// and brand logos stay as PNG URLs). Every source has a mark; the monogram
// fallback in SourceIcon is a safety net only.
import claude from './icons/claude.svg';
import codebuddy from './icons/codebuddy.png';
import codex from './icons/codex.svg';
import copilot from './icons/copilot.png';
import gemini from './icons/gemini.svg';
import hermes from './icons/hermes.svg';
import grok from './icons/grok.svg';
import antigravity from './icons/antigravity.svg';
import goose from './icons/goose.svg';
import opencode from './icons/opencode.svg';
import kilo from './icons/kilo.svg';
import cline from './icons/cline.svg';
import pi from './icons/pi.svg';
import omp from './icons/omp.svg';
import zed from './icons/zed.svg';
import workbuddy from './icons/workbuddy.png';
import qoder from './icons/qoder.svg';

const generic = 'data:image/svg+xml,%3Csvg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="%235f6880" stroke-width="2"%3E%3Cpath d="M5 3h14v18H5z"/%3E%3Cpath d="M9 8h6M9 12h6M9 16h4"/%3E%3C/svg%3E';

// Icon identities come from source-catalog.json, not Source keys. The generic
// mark makes historical or newer Ledger keys render as a Source rather than
// disappear behind a missing asset.
export const SOURCE_ICONS: Record<string, string> = {
  claude,
  codebuddy,
  codex,
  copilot,
  gemini,
  hermes,
  grok,
  antigravity,
  goose,
  opencode,
  kilo,
  cline,
  pi,
  omp,
  zed,
  workbuddy,
  qoder,
  generic,
};

export function sourceIcon(icon: string): string {
  return SOURCE_ICONS[icon] ?? SOURCE_ICONS.generic;
}
