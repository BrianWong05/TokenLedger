import { useEffect, useRef, useState, type DependencyList } from 'react';

// The supersession rule, in one place: of the requests a surface issues, only
// the LATEST may land — a superseded response never overwrites a newer
// window's figures. The overviewStore enforces the same rule with its reload
// epoch; dialogs hold their window in React state, so this is the hook-shaped
// instance the enlarges hold.
//
// `key` names the identity the figures describe (the window, the bucket); the
// value resets to null the moment it moves, so a stale figure is never shown
// against a new window. A null `fetch` fetches nothing (the placeholder
// stands, and anything in flight is superseded). A rejection leaves the
// placeholder — same as the store, a failed read shows no figure rather than
// a wrong one. `fetch` is a per-render closure over the key, deliberately not
// a dependency: the key IS the identity, and listing the closure would refire
// on every render. It rides a ref written during render (the latest-ref
// pattern) — safe because closures sharing a key are interchangeable: the key
// must name every input the fetch reads.
export function useLatest<T>(fetch: (() => Promise<T>) | null, key: DependencyList): T | null {
  const [value, setValue] = useState<T | null>(null);
  const epoch = useRef(0);
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;
  useEffect(() => {
    const mine = ++epoch.current;
    setValue(null);
    const run = fetchRef.current;
    if (!run) return;
    run().then(
      (v) => {
        if (epoch.current === mine) setValue(v);
      },
      () => {},
    );
  }, key);
  return value;
}
