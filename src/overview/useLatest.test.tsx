/** @vitest-environment jsdom */

// The supersession rule as one hook: of the requests a surface issues, only
// the LATEST may land. Before this seam the rule was hand-rolled at three
// fetch sites — TrendModal's window and bucket fetches and the Activity
// enlarge's Summary, each citing a sibling as its precedent — while the
// overviewStore keeps its own richer reload epoch.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { useLatest } from './useLatest';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function Probe({ fetch, k }: { fetch: (() => Promise<string>) | null; k: string }) {
  const value = useLatest(fetch, [k]);
  return <output>{value ?? '…'}</output>;
}

const roots: Root[] = [];
afterEach(() => {
  roots.forEach((r) => act(() => r.unmount()));
  roots.length = 0;
  document.body.innerHTML = '';
});

async function mount(fetch: (() => Promise<string>) | null, k: string) {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(<Probe fetch={fetch} k={k} />));
  return {
    read: () => container.querySelector('output')!.textContent,
    render: (f: (() => Promise<string>) | null, key: string) =>
      act(async () => root.render(<Probe fetch={f} k={key} />)),
  };
}

describe('useLatest', () => {
  it('lands the response and shows the placeholder until it does', async () => {
    const d = deferred<string>();
    const { read } = await mount(() => d.promise, 'w1');
    expect(read()).toBe('…');
    await act(async () => d.resolve('figures'));
    expect(read()).toBe('figures');
  });

  it('a superseded response never lands, even resolving after the newer one', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const { read, render } = await mount(() => first.promise, 'w1');
    await render(() => second.promise, 'w2');
    await act(async () => second.resolve('new window'));
    expect(read()).toBe('new window');
    await act(async () => first.resolve('stale window'));
    expect(read()).toBe('new window');
  });

  it('resets to the placeholder the moment the key moves', async () => {
    const first = deferred<string>();
    const { read, render } = await mount(() => first.promise, 'w1');
    await act(async () => first.resolve('old'));
    expect(read()).toBe('old');
    await render(() => deferred<string>().promise, 'w2');
    expect(read()).toBe('…');
  });

  it('a null fetch shows the placeholder and supersedes what was in flight', async () => {
    const first = deferred<string>();
    const { read, render } = await mount(() => first.promise, 'w1');
    await render(null, 'w2');
    await act(async () => first.resolve('stale'));
    expect(read()).toBe('…');
  });

  it('an unchanged key neither refires nor blanks on a new closure identity', async () => {
    let calls = 0;
    const make = () => () => {
      calls += 1;
      return Promise.resolve('figures');
    };
    const { read, render } = await mount(make(), 'w1');
    expect(read()).toBe('figures');
    await render(make(), 'w1'); // a re-render: new closure, same window
    expect(read()).toBe('figures');
    expect(calls).toBe(1);
  });

  it('a rejection leaves the placeholder', async () => {
    const d = deferred<string>();
    const { read } = await mount(() => d.promise, 'w1');
    await act(async () => d.reject(new Error('boom')));
    expect(read()).toBe('…');
  });
});
