// Kept outside Vitest's filename pattern; this exercises the Node build script.
import assert from 'node:assert/strict';
import { appendFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { buildSidecars } from './build-sidecar.mjs';

const COMPANIONS = ['antigravity-export', 'claude-limits', 'codex-limits', 'grok-limits'];

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'tokenledger-sidecars-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const tauriRoot = join(root, 'src-tauri');
  mkdirSync(join(tauriRoot, 'src', 'bin'), { recursive: true });
  writeFileSync(join(tauriRoot, 'Cargo.toml'), '[package]\nname = "fixture"\n');
  writeFileSync(join(tauriRoot, 'Cargo.lock'), '# fixture\n');
  writeFileSync(join(tauriRoot, 'build.rs'), 'fn main() {}\n');
  writeFileSync(join(tauriRoot, 'tauri.conf.json'), '{}\n');
  writeFileSync(join(tauriRoot, 'src', 'lib.rs'), 'pub fn shared() {}\n');
  for (const name of COMPANIONS) {
    writeFileSync(join(tauriRoot, 'src', 'bin', `${name}.rs`), 'fn main() {}\n');
  }

  const cargoCalls = [];
  const exe = process.platform === 'win32' ? '.exe' : '';
  const run = (command, args) => {
    if (command === 'rustc') return 'rustc 1.0.0\nhost: test-target\n';
    assert.equal(command, 'cargo');
    cargoCalls.push(args);
    const release = join(tauriRoot, 'target', 'release');
    mkdirSync(release, { recursive: true });
    for (const name of COMPANIONS) {
      writeFileSync(join(release, `${name}${exe}`), `built ${cargoCalls.length}: ${name}\n`);
    }
    return '';
  };

  return { cargoCalls, root, run, source: join(tauriRoot, 'src', 'lib.rs') };
}

test('dev builds all companions once, then skips an unchanged build', (t) => {
  const { cargoCalls, root, run } = fixture(t);

  buildSidecars({ root, ifNeeded: true, run, log: () => {} });
  assert.equal(cargoCalls.length, 1);
  assert.deepEqual(cargoCalls[0], [
    'build',
    '--release',
    '--bin',
    'antigravity-export',
    '--bin',
    'claude-limits',
    '--bin',
    'codex-limits',
    '--bin',
    'grok-limits',
    '--manifest-path',
    join(root, 'src-tauri', 'Cargo.toml'),
  ]);

  buildSidecars({ root, ifNeeded: true, run, log: () => {} });
  assert.equal(cargoCalls.length, 1);
});

test('dev rebuilds once when a Rust input changes', (t) => {
  const { cargoCalls, root, run, source } = fixture(t);

  buildSidecars({ root, ifNeeded: true, run, log: () => {} });
  appendFileSync(source, '// changed\n');
  buildSidecars({ root, ifNeeded: true, run, log: () => {} });

  assert.equal(cargoCalls.length, 2);
});

test('production builds even when the cache is current', (t) => {
  const { cargoCalls, root, run } = fixture(t);

  buildSidecars({ root, ifNeeded: false, run, log: () => {} });
  buildSidecars({ root, ifNeeded: false, run, log: () => {} });

  assert.equal(cargoCalls.length, 2);
});
