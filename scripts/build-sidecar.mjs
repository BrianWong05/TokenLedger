// Builds the companion tools and gives each the name Tauri wants for a sidecar:
// `<name>-<target-triple>`. Tauri's build calls this via `npm run sidecar`; run
// it by hand once before `tauri dev` on a fresh checkout.
//
// Both companions ship beside the app, and both stay separate executables on
// purpose: `antigravity-export` so the scan can never reach the language server
// it decrypts Sessions through (ADR-0018), and `claude-limits` so the
// always-running process provably never touches a vendor credential (ADR-0019).
// That property is checkable by grep only while these live out here.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const COMPANIONS = ['antigravity-export', 'claude-limits'];

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

// Chicken-and-egg: `tauri-build` refuses to compile the crate while ANY declared
// externalBin is missing, and the companions are targets *of* that crate. So
// every placeholder goes in first, then the real builds overwrite them. Each
// placeholder is removed again on failure so a stub can never be bundled.
const targets = COMPANIONS.map((name) => {
  const path = join(outDir, `${name}-${triple}${exe}`);
  const placeholder = !existsSync(path);
  if (placeholder) writeFileSync(path, '');
  return { name, path, placeholder };
});

for (const target of targets) {
  try {
    execFileSync(
      'cargo',
      ['build', '--release', '--bin', target.name, '--manifest-path', manifest],
      { stdio: 'inherit' },
    );
  } catch (err) {
    for (const t of targets) if (t.placeholder) rmSync(t.path, { force: true });
    throw err;
  }
  copyFileSync(join(root, 'src-tauri', 'target', 'release', `${target.name}${exe}`), target.path);
  console.log(`sidecar ready: ${target.path}`);
}
