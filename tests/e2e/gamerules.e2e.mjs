// Game rules on real servers: a 26.3 server through its management protocol while it runs and its own
// rules file while it is stopped; a 1.20.1 server through its console (and a save) while it runs and its
// level.dat while it is stopped. Each change is checked with the game's own `gamerule <name>` answer.
//
//   node --test tests/e2e/gamerules.e2e.mjs
//
// Makes its own servers and deletes them again. Linux only. As root too, as app-killed.e2e.mjs says.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;
const made = [];

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18754 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  for (const id of made) await remove(id).catch(() => {});
  await app?.stop();
});

const down = (id) => until('the server to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${id}`)).body.state.status), 180);

async function remove(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
  await client.call('DELETE', `/instances/${id}?delete_backups=true`);
}

async function server(name, mcVersion) {
  const leftover = await client.findInstance(name);
  if (leftover) await remove(leftover.id);
  const created = await client.call('POST', '/instances', { name, mc_version: mcVersion, loader: 'vanilla', max_memory_mb: 1536, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const { id } = created.body.instance;
  made.push(id);
  const job = await client.waitForJob(created.body.job_id, 900);
  assert.equal(job.status, 'succeeded', job.error);
  return id;
}

async function start(id) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
}

async function stop(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
}

const view = async (id) => (await client.call('GET', `/instances/${id}/gamerules`)).body;
const value = (rules, name) => rules.rules.find((rule) => rule.name === name)?.value;

async function change(id, changes) {
  const { status, body } = await client.call('PATCH', `/instances/${id}/gamerules`, { changes });
  assert.equal(status, 200, `${JSON.stringify(changes)}: ${JSON.stringify(body)}`);
  return body;
}

/** What the game itself says a rule is, from its answer to `gamerule <name>`. */
async function gameSays(id, name) {
  const before = (await client.console(id)).length;
  await client.call('POST', `/instances/${id}/command`, { command: `gamerule ${name}` });
  return until(`the game's answer about ${name}`, async () => {
    const lines = (await client.console(id)).slice(before);
    const answer = lines.find((line) => /is currently set to:? /.test(line) && line.includes(name));
    return answer?.replace(/.*is currently set to:? /, '').trim();
  }, 15);
}

test('26.3: live through the management protocol, and the world’s own file while stopped', { skip, timeout: 20 * 60_000 }, async (t) => {
  const id = await server('E2E gamerules', process.env.E2E_MC ?? '26.3');
  const fresh = await view(id);
  assert.equal(fresh.values, null, 'no world before the first start');
  const early = await client.call('PATCH', `/instances/${id}/gamerules`, { changes: { keep_inventory: true } });
  assert.equal(early.body.error.code, 'no_world');

  await start(id);
  try {
    const live = await view(id);
    assert.equal(live.values, 'server');
    t.diagnostic(`${live.rules.length} rules, ${live.rules.filter((rule) => rule.category === 'other').length} the catalog doesn't know`);
    assert.ok(live.rules.length >= 58);
    assert.ok(live.rules.every((rule) => rule.default === null || rule.value === rule.default), 'a new world has the defaults');

    const changed = await change(id, { keep_inventory: true, random_tick_speed: 7 });
    assert.equal(changed.via, 'management');
    assert.equal(await gameSays(id, 'keep_inventory'), 'true');
    assert.equal(await gameSays(id, 'random_tick_speed'), '7');

    const wrong = await client.call('PATCH', `/instances/${id}/gamerules`, { changes: { keep_inventory: 3 } });
    assert.equal(wrong.body.error.code, 'wrong_gamerule_type');
  } finally {
    await stop(id);
  }

  const stopped = await view(id);
  assert.equal(stopped.values, 'files');
  assert.equal(value(stopped, 'keep_inventory'), true, 'the server saved it when it stopped');
  const written = await change(id, { keep_inventory: false, spawn_phantoms: false, players_sleeping_percentage: 50 });
  assert.equal(written.via, 'files');

  await start(id);
  try {
    assert.equal(await gameSays(id, 'keep_inventory'), 'false', 'the server read what was written while it was stopped');
    assert.equal(await gameSays(id, 'spawn_phantoms'), 'false');
    assert.equal(await gameSays(id, 'players_sleeping_percentage'), '50');
    assert.equal(value(await view(id), 'random_tick_speed'), 7, 'and kept the rest');
  } finally {
    await stop(id);
  }
});

test('1.20.1: the console (then a save) while it runs, and level.dat while it is stopped', { skip, timeout: 20 * 60_000 }, async () => {
  const id = await server('E2E gamerules old', '1.20.1');
  await start(id);
  try {
    const running = await view(id);
    assert.equal(running.channel, 'console');
    assert.equal(running.values, 'last_save');
    assert.equal(running.rules.length, 45);
    assert.equal(running.rules.find((rule) => rule.name === 'doDaylightCycle').label, 'Day and night');

    const changed = await change(id, { keepInventory: true, randomTickSpeed: 5 });
    assert.equal(changed.via, 'console');
    assert.equal(value(changed.gamerules, 'keepInventory'), true, 'saved, so the file shows it');
    assert.equal(await gameSays(id, 'keepInventory'), 'true');
    assert.equal(await gameSays(id, 'randomTickSpeed'), '5');
    const typed = await client.console(id);
    assert.ok(typed.some((line) => line.endsWith('> gamerule keepInventory true')), 'typed into its console');
  } finally {
    await stop(id);
  }

  const written = await change(id, { doDaylightCycle: false, randomTickSpeed: 3 });
  assert.equal(written.via, 'files');
  await start(id);
  try {
    assert.equal(await gameSays(id, 'doDaylightCycle'), 'false', 'the server read level.dat as written');
    assert.equal(await gameSays(id, 'randomTickSpeed'), '3');
    assert.equal(await gameSays(id, 'keepInventory'), 'true', 'and kept the rest');
  } finally {
    await stop(id);
  }
});
