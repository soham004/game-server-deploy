// Real servers: installs each loader for Minecraft 26.3 through the API (real downloads and
// installers, sandboxed), starts it, checks it runs and answers, and stops it. The servers stay
// in the data folder, so a second run only starts them, and the browser suite uses them.
//
//   cargo build -p gsd-app && node --test tests/e2e/loaders.e2e.mjs
//
// Linux only (the sandbox), on a Linux filesystem. Needs the internet. Environment:
//   E2E_DATA     data folder (default ~/.cache/gsd-e2e)
//   E2E_LOADERS  which loaders (default: all seven, e.g. "paper neoforge")
//   E2E_MC       Minecraft version (default 26.3)
// Also prepares `imports/e2e-import` (a real vanilla server jar) for the browser suite's import
// scenario, from the NeoForge server's libraries.

import assert from 'node:assert/strict';
import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, test } from 'node:test';

import { DATA_DIR, startApp } from './lib/app.mjs';
import { Client } from './lib/client.mjs';

const LOADERS = (process.env.E2E_LOADERS ?? 'vanilla paper purpur fabric quilt forge neoforge').split(/\s+/).filter(Boolean);
const MC = process.env.E2E_MC ?? '26.3';
const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';

let app;
let client;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18738 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  await app?.stop();
});

for (const loader of LOADERS) {
  test(`${loader} ${MC}: installs, runs and answers`, { skip, timeout: 30 * 60_000 }, async () => {
    const id = await installed(`E2E ${loader}`, loader);
    await client.call('POST', `/instances/${id}/start`);
    const running = await client.waitForState(id, 'running', 420).catch(async (error) => {
      throw new Error(`${error.message}\n--- console ---\n${(await client.console(id)).slice(-30).join('\n')}`);
    });
    checkProcess(running.state.pid);

    assert.equal((await client.call('POST', `/instances/${id}/command`, { command: 'list' })).status, 204);
    const answered = await waitForLine(id, /players online/);
    assert.ok(answered, 'the server answered "list"');
    const lines = await client.console(id);
    assert.ok(!lines.some((line) => /[\u0000-\u0008\u000b-\u001f\u007f]/.test(line)), 'no control characters in the console');
    const diagnostics = (await client.call('GET', `/instances/${id}/console`)).body.diagnostics;
    assert.deepEqual(diagnostics, [], 'no problems detected');

    await client.call('POST', `/instances/${id}/stop`);
    await client.waitForState(id, 'stopped', 180);
  });
}

test('prepares a real vanilla server folder to import (browser suite)', { skip }, () => {
  const neoforge = join(DATA_DIR, 'instances', 'e2e-neoforge', 'server', 'libraries', 'net', 'minecraft', 'server', MC, `server-${MC}.jar`);
  if (!existsSync(neoforge)) return;
  const folder = join(DATA_DIR, 'imports', 'e2e-import');
  mkdirSync(folder, { recursive: true });
  if (!existsSync(join(folder, 'server.jar'))) copyFileSync(neoforge, join(folder, 'server.jar'));
});

/** The server named `name`, installed (created now if it doesn't exist). Returns its id. */
async function installed(name, loader) {
  const existing = await client.findInstance(name);
  if (existing?.installed) return existing.id;
  const id = existing?.id ?? await create(name, loader);
  if (existing) {
    const retry = await client.call('POST', `/instances/${id}/install`);
    await finish(retry.body.job_id);
  }
  return id;
}

async function create(name, loader) {
  const created = await client.call('POST', '/instances', { name, mc_version: MC, loader, max_memory_mb: 2048, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  await finish(created.body.job_id);
  return created.body.instance.id;
}

async function finish(jobId) {
  const job = await client.waitForJob(jobId, 25 * 60);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-15).join('\n')}`);
}

/** The server process: no new privileges (the sandbox), and its own user when the app is root. */
function checkProcess(pid) {
  if (!pid) return;
  const status = readFileSync(`/proc/${pid}/status`, 'utf8');
  assert.match(status, /^NoNewPrivs:\s+1$/m, 'no_new_privs is set');
  if (process.getuid?.() === 0) {
    const uid = status.match(/^Uid:\s+(\d+)/m)[1];
    assert.notEqual(uid, '0', 'the server runs as its own user, not root');
  }
}

async function waitForLine(id, pattern) {
  for (let i = 0; i < 40; i += 1) {
    if ((await client.console(id)).some((line) => pattern.test(line))) return true;
    await new Promise((resolve) => { setTimeout(resolve, 250); });
  }
  return false;
}
