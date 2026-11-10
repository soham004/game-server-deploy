// Real log files from a real server: the current log, the compressed one Minecraft makes of the run
// before, a real crash report (a watchdog set to give up at once), and a real JVM crash report (a
// segmentation fault sent to the running Java).
//
//   node --test tests/e2e/logs.e2e.mjs
//
// Makes its own vanilla server and deletes it again. Linux only. As root too, as app-killed.e2e.mjs says.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const MC = process.env.E2E_MC ?? '26.3';
const NAME = 'E2E logs';
const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;
let id;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18749 });
  client = new Client(app.origin);
  await client.signIn();
  const leftover = await client.findInstance(NAME);
  if (leftover) await remove(leftover.id);
  const created = await client.call('POST', '/instances', { name: NAME, mc_version: MC, loader: 'vanilla', max_memory_mb: 2048, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  id = created.body.instance.id;
  const job = await client.waitForJob(created.body.job_id, 900);
  assert.equal(job.status, 'succeeded', job.error);
});

after(async () => {
  if (client && id) await remove(id).catch(() => {});
  await app?.stop();
});

async function remove(server) {
  await client.call('POST', `/instances/${server}/stop`);
  await until('the server to stop', async () => {
    const { status } = (await client.call('GET', `/instances/${server}`)).body.state;
    return !['starting', 'running', 'stopping'].includes(status);
  }, 120);
  await client.call('DELETE', `/instances/${server}?delete_backups=true`);
}

const list = async () => (await client.call('GET', `/instances/${id}/logs`)).body;

async function read(name, query = '') {
  const { status, body } = await client.call('GET', `/instances/${id}/logs/${encodeURIComponent(name)}${query}`);
  assert.equal(status, 200, JSON.stringify(body));
  return body;
}

async function runUntilReady() {
  await client.call('POST', `/instances/${id}/start`);
  return client.waitForState(id, 'running', 300).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-20).join('\n')}`);
  });
}

async function stop() {
  await client.call('POST', `/instances/${id}/stop`);
  await client.waitForState(id, 'stopped', 120);
}

async function setProperty(key, value) {
  const { body: file } = await client.call('GET', `/instances/${id}/files/text?path=server.properties`);
  const lines = file.content.split('\n').filter((line) => !line.startsWith(`${key}=`));
  const saved = await client.call('PUT', `/instances/${id}/files/text`, { path: 'server.properties', revision: file.revision, content: [...lines, `${key}=${value}`, ''].join('\n') });
  assert.equal(saved.status, 200, JSON.stringify(saved.body));
}

test('the current log and an earlier compressed one read the same', { skip, timeout: 10 * 60_000 }, async (t) => {
  assert.deepEqual(await list(), [], 'none before the first run');
  await runUntilReady();
  await stop();
  await runUntilReady();
  await stop();

  const files = await list();
  t.diagnostic(`files: ${files.map((file) => `${file.name} (${file.kind})`).join(', ')}`);
  assert.equal(files[0].name, 'latest.log');
  const archive = files.find((file) => file.kind === 'archive');
  assert.ok(archive?.name.endsWith('.log.gz'), JSON.stringify(files));

  for (const name of ['latest.log', archive.name]) {
    const page = await read(name);
    assert.ok(page.lines.some((line) => /Done \(/.test(line.text)), `${name} has the start in it`);
    assert.ok(page.lines.some((line) => line.level === 'info'), `${name} has levels`);
    assert.equal(page.truncated, false);
    const found = await read(name, '?q=done%20(');
    assert.ok(found.lines.length >= 1 && found.lines.every((line) => /done \(/i.test(line.text)), JSON.stringify(found.lines));
  }
  const tail = await read('latest.log', '?limit=3');
  assert.equal(tail.lines.length, 3);
  assert.ok(tail.earlier > 1);
  const before = await read('latest.log', `?limit=3&before=${tail.earlier}`);
  assert.equal(before.lines.at(-1).number, tail.earlier - 1);
});

test('a real crash report is listed and summarised', { skip, timeout: 10 * 60_000 }, async (t) => {
  // The watchdog stops a server whose tick takes longer than this many milliseconds.
  await setProperty('max-tick-time', '1');
  await client.call('POST', `/instances/${id}/start`);
  const crashed = await until('the watchdog to stop the server', async () => {
    const reports = (await list()).filter((file) => file.kind === 'crash');
    return reports.length > 0 ? reports : null;
  }, 180);
  await client.waitForState(id, 'crashed', 120).catch(() => {});
  await setProperty('max-tick-time', '60000');
  await client.call('POST', `/instances/${id}/stop`);
  await until('it to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${id}`)).body.state.status), 120);

  const page = await read(crashed[0].name);
  t.diagnostic(`crash: ${JSON.stringify(page.crash)}`);
  assert.equal(page.crash.description, 'Watching Server');
  assert.match(page.crash.error, /Error: Watchdog/);
  assert.ok(page.crash.time);
  assert.ok(page.crash.frames.length > 0);
  assert.ok(page.lines.length > 10, 'and the report itself');
});

test('a real JVM crash report is listed and summarised', { skip, timeout: 10 * 60_000 }, async (t) => {
  const { state } = await runUntilReady();
  process.kill(state.pid, 'SIGSEGV');
  const reports = await until('the JVM to write its report', async () => {
    const found = (await list()).filter((file) => file.kind === 'jvm_crash');
    return found.length > 0 ? found : null;
  }, 120);
  await client.call('POST', `/instances/${id}/stop`);
  await until('it to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${id}`)).body.state.status), 120);

  const page = await read(reports[0].name);
  t.diagnostic(`crash: ${JSON.stringify(page.crash)}`);
  assert.match(page.crash.description, /fatal error/);
  assert.match(page.crash.error, /SIGSEGV/);
  assert.ok(page.crash.time, 'its time');
  assert.equal(page.crash.frames.length, 1, 'its frame');
});
