import { describe, expect, it } from 'vitest';
import { TOOL_ICONS } from './icons';
import { TOOLS, emptyByTool, sourceMeta } from './meta';

describe('Source catalog', () => {
  it('keeps lowercase pi seventh and gives it the official vendored mark', () => {
    expect(TOOLS.map((tool) => tool.key)).toEqual([
      'claude', 'codex', 'gemini', 'hermes', 'grok', 'antigravity', 'pi',
    ]);
    expect(TOOLS[TOOLS.length - 1]).toMatchObject({ key: 'pi', label: 'pi', source: 'pi' });
    expect(emptyByTool().pi).toBe(0);
    expect(TOOL_ICONS.pi).toMatch(/^data:image\/svg\+xml/);
  });

  it('derives metadata from the catalog and gives historical keys neutral fallback metadata', () => {
    expect(sourceMeta('claude')).toMatchObject({
      label: 'Claude', source: 'Claude Code', icon: 'claude', aliases: ['Claude Code'],
    });
    expect(sourceMeta('future-source')).toMatchObject({
      key: 'future-source', label: 'future-source', source: 'future-source',
      color: '#5f6880', icon: 'generic', aliases: [], capabilities: {},
    });
  });
});
