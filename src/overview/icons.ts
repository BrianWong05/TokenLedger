// Brand icons for each source, imported as asset URLs (Vite resolves *.svg to a
// URL string). Every source has a mark; the monogram fallback in ToolIcon is a
// safety net only.
import type { ToolKey } from './meta';
import claude from './icons/claude.svg';
import codex from './icons/codex.svg';
import gemini from './icons/gemini.svg';
import hermes from './icons/hermes.svg';
import grok from './icons/grok.svg';
import antigravity from './icons/antigravity.svg';
import pi from './icons/pi.svg';

export const TOOL_ICONS: Partial<Record<ToolKey, string>> = {
  claude,
  codex,
  gemini,
  hermes,
  grok,
  antigravity,
  pi,
};
