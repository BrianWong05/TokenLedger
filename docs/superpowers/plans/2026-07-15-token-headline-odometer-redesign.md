# Token Headline Odometer Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the weak, overlapping headline transition with a clipped 1.4-second multi-turn odometer animation.

**Architecture:** `TokenTotalHeadline` continues to own mode state and the animation lifecycle. `RollingTokenTotal` changes from three independently positioned layers to one centered slot grid inside a width-morphing clipped viewport; CSS animates multi-glyph reels and symbol fades within those slots.

**Tech Stack:** React 18, TypeScript, CSS keyframes, Vitest with jsdom.

## Global Constraints

- Keep all mode-change animation inside the large Total tokens headline.
- Use a 1,400 ms total duration and approximately 55 ms left-to-right settling.
- Roll format-only changes upward through at least one complete digit cycle.
- Prevent overlap with one clipped viewport and one aligned slot grid.
- Preserve persistence, accessibility, responsive layout, tabular numerals, identical-string no-op motion, and Reduce Motion.
- Do not change backend, Ledger, query, Cost, or domain behavior.

---

### Task 1: Replace layered motion with a clipped odometer grid

**Files:**
- Modify: `src/overview/TokenTotalHeadline.test.tsx`
- Modify: `src/overview/TokenTotalHeadline.tsx`
- Modify: `src/overview/overview.css`

**Interfaces:**
- Consumes: `TokenTotalHeadline({ total: number })`, `formatCompactTokenTotal(total)`, and `formatExactTokenTotal(total)`.
- Produces: the same semantic button and final visible strings, with a single clipped animated child while `aria-busy="true"`.

- [x] **Step 1: Write the failing overlap and duration regression**

```tsx
it('keeps exact-to-compact motion inside one clipped viewport for 1.4 seconds', () => {
  vi.useFakeTimers();
  localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
  const button = renderHeadline(5_841_112_112);

  act(() => button.click());
  const viewport = button.firstElementChild as HTMLElement;
  expect(getComputedStyle(viewport).overflow).toBe('hidden');
  expect(viewport.childElementCount).toBe(1);
  act(() => vi.advanceTimersByTime(1_399));
  expect(button.getAttribute('aria-busy')).toBe('true');
  act(() => vi.advanceTimersByTime(1));
  expect(button.textContent).toBe('5.84B');
});
```

- [x] **Step 2: Run the focused test and verify the current renderer fails**

Run: `npm test -- src/overview/TokenTotalHeadline.test.tsx`

Expected: FAIL because the current animated root does not clip overflow, has three independent children, and settles at 800 ms.

- [x] **Step 3: Implement aligned slots and full digit reels**

Use centered source/target arrays and continuous upward digit sequences:

```tsx
function centeredCharacters(value: string, length: number) {
  const characters = Array.from(value);
  const leftPadding = Math.floor((length - characters.length) / 2);
  return Array.from({ length }, (_, index) => characters[index - leftPadding] ?? null);
}

function upwardDigitReel(from: string | null, to: string) {
  const start = from !== null && isDigit(from) ? Number(from) : 0;
  const target = Number(to);
  const steps = 10 + ((target - start + 10) % 10);
  return Array.from({ length: steps + 1 }, (_, index) => String((start + index) % 10));
}
```

Render one `.tt-token-odometer-viewport` with inline `overflow: hidden`, one `.tt-token-odometer-grid`, and one clipped `.tt-token-odometer-slot` for every aligned character position. Target digits receive a full reel; unmatched source digits receive a full exit reel; symbols crossfade within their slot. Change `MODE_ANIMATION_MS` to `1_400` and `DIGIT_SETTLE_STAGGER_MS` to `55`.

- [x] **Step 4: Replace layered CSS with viewport, slot, reel, and symbol keyframes**

```css
.tt-token-odometer-viewport {
  position: relative;
  display: inline-block;
  height: 1em;
  overflow: hidden;
  animation: tt-token-width-morph 1400ms cubic-bezier(0.2, 0.72, 0.18, 1) forwards;
}
.tt-token-odometer-slot {
  position: relative;
  width: 1ch;
  height: 1em;
  overflow: hidden;
}
.tt-token-odometer-reel {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  animation: tt-token-odometer-up var(--tt-roll-duration)
    cubic-bezier(0.16, 0.72, 0.16, 1) forwards;
}
```

Add Reduce Motion fallbacks that show the final reel glyph, remove exiting reels and old symbols, expose new symbols immediately, and set the viewport to its target width.

- [x] **Step 5: Run focused tests and production typechecking**

Run: `npm test -- src/overview/TokenTotalHeadline.test.tsx && npm run build`

Expected: all component tests pass and the production build exits 0.

- [x] **Step 6: Run full verification and commit**

Run: `npm test && npm run build && git diff --check`

Expected: all frontend tests pass, the production build exits 0, and the diff check is clean.

```bash
git add src/overview/TokenTotalHeadline.test.tsx src/overview/TokenTotalHeadline.tsx src/overview/overview.css docs/superpowers/specs/2026-07-15-token-headline-odometer-redesign.md docs/superpowers/plans/2026-07-15-token-headline-odometer-redesign.md
git commit -m "fix(overview): strengthen token odometer animation"
```
