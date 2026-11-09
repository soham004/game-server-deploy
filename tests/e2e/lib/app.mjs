// Starts the real app binary on a data folder, for the scripts that drive a running app
// (Linux only: the sandbox needs it).
//
//   const app = await startApp({ dataDir, port: 18742 });
//   … app.origin …
//   await app.stop();
//
// The binary is `$GSD_BIN`, or `$CARGO_TARGET_DIR/debug/gsd` (default `target/debug/gsd`): build
// it first with `cargo build -p gsd-app`.

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

import { until } from './client.mjs';

/** Where the real-server tests keep their servers between runs (must be a Linux filesystem). */
export const DATA_DIR = process.env.E2E_DATA ?? join(homedir(), '.cache', 'gsd-e2e');

export function appBinary() {
  return process.env.GSD_BIN ?? resolve(process.env.CARGO_TARGET_DIR ?? 'target', 'debug', 'gsd');
}

/**
 * Runs `gsd serve` until `stop()`. The app's own output is kept (`log()`) for failure reports.
 * `kill()` ends it with SIGKILL instead, as a crash or the out-of-memory killer would: it gets no
 * chance to stop its servers.
 * @returns {Promise<{ origin: string, log: () => string, stop: () => Promise<void>, kill: () => Promise<void> }>}
 */
export async function startApp({ dataDir = DATA_DIR, port }) {
  const bin = appBinary();
  if (!existsSync(bin)) throw new Error(`${bin} does not exist: run \`cargo build -p gsd-app\` (or set GSD_BIN)`);
  const output = [];
  const child = spawn(bin, ['--data-dir', dataDir, '--bind', `127.0.0.1:${port}`, 'serve'], { stdio: ['ignore', 'pipe', 'pipe'] });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', (chunk) => output.push(chunk.toString()));
  const exited = new Promise((resolve) => { child.on('exit', resolve); });
  const origin = `http://127.0.0.1:${port}`;
  const log = () => output.join('').replace(/\x1b\[[0-9;]*m/g, '');
  try {
    await until('the app to answer', async () => (await fetch(`${origin}/api/v1/health`)).ok, 60);
  } catch (error) {
    child.kill();
    throw new Error(`${error.message}\n--- app output ---\n${log()}`);
  }
  return {
    origin,
    log,
    async stop() {
      child.kill('SIGTERM');
      await exited;
    },
    async kill() {
      child.kill('SIGKILL');
      await exited;
    },
  };
}
