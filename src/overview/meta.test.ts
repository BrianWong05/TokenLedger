import { describe, expect, it } from 'vitest';
import { SOURCE_ICONS } from './icons';
import { SOURCES, emptyBySource, sourceMeta } from './meta';

describe('Source catalog', () => {
  it('keeps Cline, Goose, OpenCode, and lowercase pi in catalog order with distinct marks', () => {
    expect(SOURCES.map((source) => source.key)).toEqual([
      'claude', 'codex', 'gemini', 'hermes', 'grok', 'antigravity', 'goose', 'opencode', 'cline', 'pi',
    ]);
    expect(SOURCES[SOURCES.length - 1]).toMatchObject({ key: 'pi', label: 'pi', source: 'pi' });
    expect(emptyBySource().pi).toBe(0);
    expect(sourceMeta('opencode')).toMatchObject({
      key: 'opencode', label: 'OpenCode', source: 'OpenCode', icon: 'opencode', aliases: ['OpenCode CLI'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'db', path: '.local/share/opencode/opencode.db' }),
      ]),
    });
    expect(sourceMeta('goose')).toMatchObject({
      key: 'goose', label: 'Goose', source: 'Goose', icon: 'goose', aliases: ['Block Goose'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
    });
    expect(sourceMeta('cline')).toMatchObject({
      key: 'cline', label: 'Cline', source: 'Cline', icon: 'cline', aliases: ['Cline CLI', 'Cline VS Code'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'cli-default-data', path: '.cline/data' }),
        expect.objectContaining({ id: 'cli-data', environment: 'CLINE_DATA_DIR' }),
      ]),
    });
    expect(SOURCE_ICONS.goose).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.opencode).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.cline).toMatch(/^data:image\/svg\+xml/);
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
