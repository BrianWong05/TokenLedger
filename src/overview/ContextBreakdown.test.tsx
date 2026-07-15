/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import ContextBreakdown from './ContextBreakdown';
import { TOOLS } from './meta';
import type { CtxTotals } from './data';
import type { CtxExecRow } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mountedRoots: Root[] = [];

// Estimated Tool-calls total = 1000, allocated entirely to the single Execution
// → Bash leaf, so the leaf's exec facets have a nonzero bashTotal to split.
const ctx: CtxTotals = {
  billed: 1000, reused: 500,
  messages: 100, system: 50, reasoning: null,
  toolcalls: 1000, agents: null, mcp: null, skills: null,
};
const tree = [{ label: 'Execution', tokens: 1000, tools: [{ name: 'Bash', tokens: 1000, calls: 5 }] }];
// kind/exe/cmd deliberately distinct per row so each exec tab renders a
// different key set.
const execRows: CtxExecRow[] = [
  { source: 'claude', kind: 'vcs', exe: 'git', cmd: 'git status', estTokens: 400, calls: 3 },
  { source: 'claude', kind: 'build', exe: 'npm', cmd: 'npm test', estTokens: 600, calls: 2 },
];

function render() {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  act(() =>
    root.render(
      <ContextBreakdown tool={TOOLS[0]} ctx={ctx} view={null} tree={tree} meta="" execRows={execRows} />,
    ),
  );
  return container;
}

function queryRow(container: HTMLElement, label: string): HTMLElement | undefined {
  return Array.from(container.querySelectorAll<HTMLElement>('.tt-ctx-row')).find((r) =>
    r.querySelector('.name')?.textContent?.includes(label),
  );
}

function clickRow(container: HTMLElement, label: string) {
  const row = queryRow(container, label);
  if (!row) throw new Error(`row not found: ${label}`);
  act(() => row.click());
}

function expandToBashLeaf(container: HTMLElement) {
  clickRow(container, 'Tool calls');
  clickRow(container, 'Execution');
  clickRow(container, 'Bash');
}

function execKeys(container: HTMLElement): string[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>('.tt-exec-table .tr .k'),
    (k) => k.textContent ?? '',
  );
}

function clickTab(container: HTMLElement, label: string) {
  const btn = Array.from(
    container.querySelectorAll<HTMLButtonElement>('.tt-exec-tabs button'),
  ).find((b) => b.textContent === label);
  if (!btn) throw new Error(`tab not found: ${label}`);
  act(() => btn.click());
}

describe('ContextBreakdown drilldown', () => {
  afterEach(() => {
    for (const root of mountedRoots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.replaceChildren();
  });

  it('reveals the tree node only after Tool calls is expanded', () => {
    const c = render();
    expect(queryRow(c, 'Execution')).toBeUndefined();

    clickRow(c, 'Tool calls');
    expect(queryRow(c, 'Execution')).toBeDefined();
    expect(queryRow(c, 'Bash')).toBeUndefined();

    clickRow(c, 'Execution');
    expect(queryRow(c, 'Bash')).toBeDefined();
  });

  it('renders the exec facet table when the Bash leaf is expanded', () => {
    const c = render();
    expect(c.querySelector('.tt-exec')).toBeNull();

    expandToBashLeaf(c);

    expect(c.querySelector('.tt-exec')).not.toBeNull();
    expect(execKeys(c)).toEqual(['build', 'vcs']); // default "By type" tab
  });

  it('switches the rendered rows when the exec tab changes', () => {
    const c = render();
    expandToBashLeaf(c);
    expect(execKeys(c)).toEqual(['build', 'vcs']);

    clickTab(c, 'Executable');
    expect(execKeys(c)).toEqual(['npm', 'git']);

    clickTab(c, 'Command');
    expect(execKeys(c)).toEqual(['npm test', 'git status']);
  });
});
