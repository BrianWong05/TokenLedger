import { describe, expect, it } from 'vitest';
import { SOURCE_ICONS } from './icons';
import { SOURCES, emptyBySource, sourceMeta } from './meta';

describe('Source catalog', () => {
  it('keeps lowercase pi seventh and gives it the official vendored mark', () => {
    expect(SOURCES.map((source) => source.key)).toEqual([
      'claude', 'codex', 'gemini', 'hermes', 'grok', 'antigravity', 'pi',
    ]);
    expect(SOURCES[SOURCES.length - 1]).toMatchObject({ key: 'pi', label: 'pi', source: 'pi' });
    expect(emptyBySource().pi).toBe(0);
    expect(SOURCE_ICONS.pi).toMatch(/^data:image\/svg\+xml/);
  });

  it('derives metadata from the catalog and gives historical keys neutral fallback metadata', () => {
    expect(sourceMeta('claude')).toMatchObject({
      label: 'Claude', source: 'Claude Code', icon: 'claude', aliases: ['Claude Code'],
      artifacts: [expect.objectContaining({ id: 'projects', path: '.claude/projects', platforms: ['all'] })],
      platforms: ['all'], prerequisite: null,
    });
    expect(sourceMeta('future-source')).toMatchObject({
      key: 'future-source', label: 'future-source', source: 'future-source',
      color: '#5f6880', icon: 'generic', aliases: [], capabilities: {},
      artifacts: [], platforms: [], prerequisite: null,
    });
  });
});
