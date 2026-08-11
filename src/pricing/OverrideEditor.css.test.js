import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('OverrideEditor stylesheet', () => {
  it('imports its own CSS, so the Overview entry is styled without the Pricing chunk', () => {
    // The Overview mounts this editor eagerly while PricingPage — and, when the
    // import lived only there, its stylesheet — is lazy. jsdom applies no CSS,
    // so the check is on the source.
    const src = readFileSync(resolve(process.cwd(), 'src/pricing/OverrideEditor.tsx'), 'utf8');
    expect(src).toMatch(/^import '\.\/pricing\.css';$/m);
  });
});
