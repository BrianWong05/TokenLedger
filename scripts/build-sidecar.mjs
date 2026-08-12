// Builds the companion tools and gives each the name Tauri wants for a sidecar:
// `<name>-<target-triple>`. Production builds always rebuild them; dev builds
// use a content cache so an unchanged startup does not compile release binaries.
//
// The companions stay separate executables on purpose: `antigravity-export`
// keeps the language server outside the scan (ADR-0018), while the Limits
// companions keep vendor credentials outside the always-running app (ADR-0019).
// That property is checkable by grep only while these live out here.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const COMPANIONS = ['antigravity-export', 'antigravity-limits', 'claude-limits', 'codex-limits', 'grok-limits'];
const CACHE_VERSION = 1;
const DEFAULT_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

function filesUnder(path) {
  if (!existsSync(path)) return [];
  if (statSync(path).isFile()) return [path];
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const child = join(path, entry.name);
    if (entry.isDirectory()) return filesUnder(child);
    return entry.isFile() ? [child] : [];
  });
}

function buildInputs(root) {
  const tauriRoot = join(root, 'src-tauri');
  return [
    join(root, '.cargo'),
    join(root, 'scripts', 'build-sidecar.mjs'),
    join(tauriRoot, '.cargo'),
    join(tauriRoot, 'Cargo.toml'),
    join(tauriRoot, 'Cargo.lock'),
    join(tauriRoot, 'build.rs'),
    join(tauriRoot, 'capabilities'),
    join(tauriRoot, 'src'),
    join(tauriRoot, 'tauri.conf.json'),
  ]
    .flatMap(filesUnder)
    .sort();
}

function fingerprint(root, rustcVersion) {
  const hash = createHash('sha256');
  hash.update(`sidecar-cache-v${CACHE_VERSION}\0${process.platform}\0${process.arch}\0`);
  hash.update(rustcVersion);
  for (const [name, value] of Object.entries(process.env).sort()) {
    if (
      name === 'RUSTFLAGS' ||
      name === 'MACOSX_DEPLOYMENT_TARGET' ||
      name === 'CARGO_BUILD_TARGET' ||
      name.startsWith('CARGO_PROFILE_RELEASE_') ||
      name.startsWith('CARGO_TARGET_')
    ) {
      hash.update(`\0env:${name}=${value ?? ''}`);
    }
  }
  for (const path of buildInputs(root)) {
    hash.update(`\0file:${relative(root, path)}\0`);
    hash.update(readFileSync(path));
  }
  return hash.digest('hex');
}

function outputHashes(targets) {
  const hashes = {};
  for (const target of targets) {
    if (!existsSync(target.path) || statSync(target.path).size === 0) return null;
    hashes[target.name] = createHash('sha256').update(readFileSync(target.path)).digest('hex');
  }
  return hashes;
}

function readCache(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
}

export function buildSidecars({
  root = DEFAULT_ROOT,
  ifNeeded = false,
  run = execFileSync,
  log = console.log,
} = {}) {
  const tauriRoot = join(root, 'src-tauri');
  const manifest = join(tauriRoot, 'Cargo.toml');
  const rustcVersion = run('rustc', ['-vV'], { encoding: 'utf8' });
  const host = rustcVersion.split('\n').find((line) => line.startsWith('host: '));
  if (!host) throw new Error('cannot read the Rust host triple from `rustc -vV`');
  const triple = host.slice('host: '.length).trim();
  const exe = process.platform === 'win32' ? '.exe' : '';

  const outDir = join(tauriRoot, 'binaries');
  const cachePath = join(outDir, '.sidecar-build-cache.json');
  mkdirSync(outDir, { recursive: true });
  const targets = COMPANIONS.map((name) => ({
    name,
    path: join(outDir, `${name}-${triple}${exe}`),
  }));
  const sourceFingerprint = fingerprint(root, rustcVersion);

  if (ifNeeded) {
    const cache = readCache(cachePath);
    const hashes = outputHashes(targets);
    if (
      cache?.version === CACHE_VERSION &&
      cache.fingerprint === sourceFingerprint &&
      hashes &&
      COMPANIONS.every((name) => hashes[name] === cache.outputs?.[name])
    ) {
      log('sidecars unchanged; skipping release build');
      return { built: false, targets };
    }
  }

  // Chicken-and-egg: `tauri-build` refuses to compile the crate while ANY
  // declared externalBin is missing, and the companions are targets *of* that
  // crate. Put every placeholder in first, and remove new ones if Cargo fails.
  for (const target of targets) {
    target.placeholder = !existsSync(target.path);
    if (target.placeholder) writeFileSync(target.path, '');
  }

  try {
    run(
      'cargo',
      [
        'build',
        '--release',
        ...COMPANIONS.flatMap((name) => ['--bin', name]),
        '--manifest-path',
        manifest,
      ],
      { stdio: 'inherit' },
    );
  } catch (err) {
    for (const t of targets) if (t.placeholder) rmSync(t.path, { force: true });
    throw err;
  }

  for (const target of targets) {
    copyFileSync(join(tauriRoot, 'target', 'release', `${target.name}${exe}`), target.path);
    log(`sidecar ready: ${target.path}`);
  }

  writeFileSync(
    cachePath,
    `${JSON.stringify({
      version: CACHE_VERSION,
      fingerprint: sourceFingerprint,
      outputs: outputHashes(targets),
    })}\n`,
  );
  return { built: true, targets };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  buildSidecars({ ifNeeded: process.argv.includes('--if-needed') });
}
