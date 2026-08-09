// Builds the `antigravity-export` companion and gives it the name Tauri wants
// for a sidecar: `<name>-<target-triple>`. Tauri's build calls this via
// `npm run sidecar`; run it by hand once before `tauri dev` on a fresh checkout.
//
// The companion ships beside the app so any install can decrypt its own
// Sessions (ADR-0018). It stays a separate executable on purpose: the scan can
// never reach it, which is what keeps ADR-0013's passive boundary honest.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = join(root, 'src-tauri', 'Cargo.toml');

const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' })
  .split('\n')
  .find((line) => line.startsWith('host: '));
if (!host) throw new Error('cannot read the Rust host triple from `rustc -vV`');
const triple = host.slice('host: '.length).trim();
const exe = process.platform === 'win32' ? '.exe' : '';

const outDir = join(root, 'src-tauri', 'binaries');
mkdirSync(outDir, { recursive: true });
const target = join(outDir, `antigravity-export-${triple}${exe}`);

// Chicken-and-egg: `tauri-build` refuses to compile the crate while a declared
// externalBin is missing, and the companion is a target *of* that crate. A
// placeholder satisfies the check long enough to build the real thing, which
// then overwrites it. Removed again on failure so a stub can never be bundled.
const placeholder = !existsSync(target);
if (placeholder) writeFileSync(target, '');
try {
  execFileSync(
    'cargo',
    ['build', '--release', '--bin', 'antigravity-export', '--manifest-path', manifest],
    { stdio: 'inherit' },
  );
} catch (err) {
  if (placeholder) rmSync(target, { force: true });
  throw err;
}

copyFileSync(join(root, 'src-tauri', 'target', 'release', `antigravity-export${exe}`), target);
console.log(`sidecar ready: ${target}`);
