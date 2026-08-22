import { describe, expect, it } from 'vitest';
import type { LedgerContext } from '../types';
import { sourceContext, sourceTotals } from './context';

function ctx(over: Partial<LedgerContext> = {}): LedgerContext {
  return {
    resources: [],
    buckets: [],
    tools: [],
    skills: [],
    exec: [],
    totals: [],
    ...over,
  };
}

describe('sourceTotals', () => {
  it('keeps an unattributable category as null, not zero', () => {
    const t = sourceTotals(
      ctx({
        totals: [{
          source: 'claude', billed: 170, reused: 20,
          messages: 1000, system: 90, reasoning: null,
          toolcalls: 300, agents: null, mcp: 40, skills: 10,
        }],
      }),
      'claude',
    );
    expect(t.billed).toBe(170);
    expect(t.messages).toBe(1000);
    expect(t.reasoning).toBeNull();
    expect(t.agents).toBeNull();
  });

  it('an unknown Source is billed zero with every category unattributable', () => {
    expect(sourceTotals(ctx(), 'hermes')).toEqual({
      billed: 0, reused: 0,
      messages: null, system: null, reasoning: null,
      toolcalls: null, agents: null, mcp: null, skills: null,
    });
  });
});

describe('sourceContext', () => {
  const ledger = ctx({
    resources: [
      { source: 'claude', kind: 'skill', name: 'grilling' },
      { source: 'codex', kind: 'mcp_server', name: 'pencil' },
    ],
    buckets: [{
      source: 'claude', history: 800, newInput: 100, system: 50, response: 40, reasoning: null,
    }],
    tools: [
      { source: 'claude', name: 'Read', estTokens: 600, calls: 4 },
      { source: 'claude', name: 'mcp__github__list_issues', estTokens: 300, calls: 2 },
      { source: 'claude', name: 'Bash', estTokens: 300, calls: 3 },
    ],
    skills: [
      { source: 'claude', name: 'brainstorming', estTokens: 150, uses: 1 },
      { source: 'claude', name: 'verify', estTokens: 80, uses: 2 },
    ],
    exec: [
      { source: 'claude', kind: 'bash', exe: 'git', cmd: 'git commit', estTokens: 40, calls: 3 },
    ],
    totals: [{
      source: 'claude', billed: 1210, reused: 800,
      messages: 8000, system: 1200, reasoning: null,
      toolcalls: 900, agents: null, mcp: 300, skills: 150,
    }],
  });

  it('builds the card from the readout, not from series', () => {
    const card = sourceContext(ledger, 'claude');
    expect(card.totals.toolcalls).toBe(900);
    expect(card.totals.agents).toBeNull();
    expect(card.view?.messages).toBe(940);
    expect(card.tree.reduce((a, c) => a + c.tokens, 0)).toBe(900);
    expect(card.mcp[0]?.name).toBe('github');
    expect(card.mcp.reduce((a, m) => a + m.tokens, 0)).toBe(300);
    expect(card.meta).toContain('1 skill');
    expect(card.signatures).toHaveLength(1);
    expect(card.signatures[0]).toMatchObject({ exe: 'git', cmd: 'git commit' });
  });

  it('keeps every skill on the report path while the card may fold', () => {
    const many = ctx({
      ...ledger,
      skills: Array.from({ length: 12 }, (_, i) => ({
        source: 'claude', name: `skill-${i}`, estTokens: 12 - i, uses: 1,
      })),
    });
    const card = sourceContext(many, 'claude');
    expect(card.skillRows).toHaveLength(12);
    expect(card.skills.some((s) => s.rest > 0)).toBe(true);
  });

  it('a Source that cannot attribute Context yields no tree and no "—"-as-zero', () => {
    const card = sourceContext(ledger, 'hermes');
    expect(card.totals.messages).toBeNull();
    expect(card.tree).toEqual([]);
    expect(card.skills).toEqual([]);
    expect(card.mcp).toEqual([]);
    expect(card.meta).toBe('');
  });
});
