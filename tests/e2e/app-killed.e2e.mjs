// The app is killed outright (SIGKILL, as a crash or the out-of-memory killer would) while a real
// server runs. The server must save its world and exit by itself, and start again under a new app:
// a Java left behind would hold `session.lock` and refuse every later start (D-047).
//
//   node --test tests/e2e/app-killed.e2e.mjs
//
// Makes its own vanilla server and deletes it again. Linux only. Run it as root as well, for the case
// where the server runs as a different user from the app. Other users must be able to enter the data
// folder and run the binary, so keep both out of /root and private home folders:
//
//   E2E_DATA=/var/tmp/gsd-e2e-root GSD_BIN=/var/tmp/gsd-e2e-bin/gsd node --test tests/e2e/app-killed.e2e.mjs

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const MC = process.env.E2E_MC ?? '26.3';
const PORT = 18746;
const NAME = 'E2E app killed';
const BLOCK = '0 100 0';
const FOUND = 'block-still-there';
const skip = process.platform === 'linux' ? false : 'the signal this tests is Linux only';

/** Whether a process exists and has not finished (a finished one may wait as a zombie for a while). */
function isRunning(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
    return stat.slice(stat.lastIndexOf(')') + 2, stat.lastIndexOf(')') + 3) !== 'Z';
  } catch {
    return false;
  }
}

async function signedIn(app) {
  const client = new Client(app.origin);
  await client.signIn();
  return client;
}

async function start(client, id) {
  await client.call('POST', `/instances/${id}/start`);
  return client.waitForState(id, 'running', 300).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-20).join('\n')}`);
  });
}

async function remove(client, id) {
  await client.call('POST', `/instances/${id}/stop`);
  await until('the server to stop', async () => {
    const { status } = (await client.call('GET', `/instances/${id}`)).body.state;
    return !['starting', 'running', 'stopping'].includes(status);
  }, 120);
  const deleted = await client.call('DELETE', `/instances/${id}?delete_backups=true`);
  assert.equal(deleted.status, 204, JSON.stringify(deleted.body));
}

async function send(client, id, command) {
  const sent = await client.call('POST', `/instances/${id}/command`, { command });
  assert.ok(sent.status < 300, `${command}: ${sent.status} ${JSON.stringify(sent.body)}`);
}

async function lastLogLines(client, id) {
  const log = await client.call('GET', `/instances/${id}/files/text?path=logs/latest.log`);
  assert.equal(log.status, 200, JSON.stringify(log.body));
  return log.body.content.trimEnd().split('\n').slice(-5).join('\n');
}

test('a server saves and exits when the app is killed, and starts again afterwards', { skip, timeout: 20 * 60_000 }, async (t) => {
  let app = await startApp({ port: PORT });
  let client = await signedIn(app);
  const leftover = await client.findInstance(NAME);
  if (leftover) await remove(client, leftover.id);
  const created = await client.call('POST', '/instances', { name: NAME, mc_version: MC, loader: 'vanilla', max_memory_mb: 2048, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const { id } = created.body.instance;
  let pid;
  let killed = false;
  try {
    const job = await client.waitForJob(created.body.job_id, 900);
    assert.equal(job.status, 'succeeded', job.error);

    pid = (await start(client, id)).state.pid;
    assert.ok(Number.isInteger(pid) && isRunning(pid), `the server has a process: ${pid}`);

    // Placed seconds before the kill, long before an autosave (every 5 minutes): only saving on the
    // way out can keep it.
    await send(client, id, 'forceload add 0 0');
    await send(client, id, `setblock ${BLOCK} minecraft:gold_block`);
    await until('the block to be placed', async () => (await client.console(id)).some((line) => line.includes('Changed the block')), 20);

    await app.kill();
    killed = true;
    await until(`the server (${pid}) to exit after the app was killed`, async () => !isRunning(pid), 60);

    app = await startApp({ port: PORT });
    client = await signedIn(app);
    killed = false;
    t.diagnostic(`log after the kill:\n${await lastLogLines(client, id)}`);
    assert.equal((await client.call('GET', `/instances/${id}`)).body.state.status, 'stopped');

    await start(client, id);
    t.diagnostic('started again under the new app');

    await send(client, id, `execute if block ${BLOCK} minecraft:gold_block run say ${FOUND}`);
    await until('the block placed before the kill to be found', async () => (await client.console(id)).some((line) => line.includes(`[Server] ${FOUND}`)), 20)
      .catch(async (error) => {
        throw new Error(`the world was not saved on the way out: ${error.message}\n${(await client.console(id)).slice(-10).join('\n')}`);
      });

    await client.call('POST', `/instances/${id}/stop`);
    await client.waitForState(id, 'stopped', 120);
    t.diagnostic(`log after a normal stop:\n${await lastLogLines(client, id)}`);
  } finally {
    // After a failure the Java may be the very thing this test looks for: don't leave it running.
    if (pid && isRunning(pid)) process.kill(pid, 'SIGKILL');
    if (killed) {
      app = await startApp({ port: PORT });
      client = await signedIn(app);
    }
    // Reported, not thrown: an error here would replace the one that says why the test failed.
    await remove(client, id).catch((error) => t.diagnostic(`cleaning up: ${error.message}`));
    await app.stop();
  }
});
