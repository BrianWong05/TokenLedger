import { describe, expect, it } from 'vitest';
import { SOURCE_ICONS } from './icons';
import { SOURCES, emptyBySource, sourceMeta } from './meta';

describe('Source catalog', () => {
  it('keeps Zed, Kilo, Cline, Goose, OpenCode, lowercase pi, WorkBuddy, CodeBuddy, and Qoder in catalog order with distinct marks', () => {
    expect(SOURCES.map((source) => source.key)).toEqual([
      'claude', 'codex', 'copilot', 'gemini', 'hermes', 'grok', 'antigravity', 'goose', 'opencode', 'kilo', 'zed', 'cline', 'pi', 'omp', 'workbuddy', 'codebuddy', 'qoder',
    ]);
    expect(SOURCES[SOURCES.length - 1]).toMatchObject({ key: 'qoder', label: 'Qoder', source: 'Qoder', icon: 'qoder' });
    expect(sourceMeta('copilot')).toMatchObject({ key: 'copilot', icon: 'copilot' });
    expect(SOURCE_ICONS.copilot).not.toBe(SOURCE_ICONS.generic);
    expect(emptyBySource().pi).toBe(0);
    expect(emptyBySource().kilo).toBe(0);
    expect(emptyBySource().omp).toBe(0);
    expect(sourceMeta('omp')).toMatchObject({
      key: 'omp', label: 'Oh My Pi', source: 'Oh My Pi', icon: 'omp', aliases: ['Oh My Pi', 'omp'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: true },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'sessions', path: '.omp/agent/sessions' }),
      ]),
    });
    expect(sourceMeta('codebuddy')).toMatchObject({
      key: 'codebuddy', label: 'CodeBuddy', source: 'CodeBuddy', icon: 'codebuddy', aliases: ['CodeBuddy CLI', 'CodeBuddy IDE', 'CodeBuddy VS Code'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'projects', path: '.codebuddy/projects' }),
      ]),
    });
    expect(sourceMeta('qoder')).toMatchObject({
      key: 'qoder', label: 'Qoder', source: 'Qoder', icon: 'qoder', aliases: ['Qoder IDE', 'Qoder CLI', 'Qoder CN'],
      // CLI transcripts attribute context (reasoning) since parser v4.
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: true },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'db-cn-macos', path: 'Library/Application Support/QoderCN/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'db-cn-linux', path: '.config/QoderCN/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'db-cn-windows', path: 'AppData/Roaming/QoderCN/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'db-qoder-macos', path: 'Library/Application Support/Qoder/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'db-qoder-linux', path: '.config/Qoder/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'db-qoder-windows', path: 'AppData/Roaming/Qoder/SharedClientCache/cache/db/local.db' }),
        expect.objectContaining({ id: 'qoder-projects', path: '.qoder/projects' }),
        expect.objectContaining({ id: 'qoder-cli-projects', path: '.qoder-cli/projects' }),
        expect.objectContaining({ id: 'qoder-cn-projects', path: '.qoder-cn/projects' }),
      ]),
    });
    expect(sourceMeta('workbuddy')).toMatchObject({
      key: 'workbuddy', label: 'WorkBuddy', source: 'WorkBuddy', icon: 'workbuddy', aliases: ['WorkBuddy desktop'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'projects', path: '.workbuddy/projects' }),
      ]),
    });
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
    expect(sourceMeta('kilo')).toMatchObject({
      key: 'kilo', label: 'Kilo', source: 'Kilo CLI', icon: 'kilo', aliases: ['Kilo Code', 'Kilo Code CLI'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'db-macos', path: 'Library/Application Support/kilo/kilo.db' }),
        expect.objectContaining({ id: 'db-linux', path: '.local/share/kilo/kilo.db' }),
      ]),
    });
    expect(sourceMeta('zed')).toMatchObject({
      key: 'zed', label: 'Zed', source: 'Zed', icon: 'zed', aliases: ['Zed Editor'],
      capabilities: { model: true, project: true, session: true, tokenCategories: true, context: false },
      platforms: ['linux', 'macos', 'windows'],
      artifacts: expect.arrayContaining([
        expect.objectContaining({ id: 'database-macos', path: 'Library/Application Support/Zed/threads/threads.db' }),
        expect.objectContaining({ id: 'database-linux', path: '.local/share/zed/threads/threads.db' }),
        expect.objectContaining({ id: 'database-windows', path: 'AppData/Local/Zed/threads/threads.db' }),
      ]),
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
    expect(SOURCE_ICONS.kilo).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.cline).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.pi).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.zed).toMatch(/^data:image\/svg\+xml/);
    expect(SOURCE_ICONS.omp).toMatch(/^data:image\/svg\+xml/);
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
