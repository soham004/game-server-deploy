// Console autocomplete on real servers: the vanilla data generator run in the server's sandbox (26.3),
// every command the game's own `help` lists completed, ids from the game's registries and data pack, a
// completed command the game accepts; Paper's and NeoForge's own commands, and a plugin's and a mod's
// (Chunky, from Modrinth), learned from their `help`.
//
//   node --test tests/e2e/commands.e2e.mjs
//
// Uses "E2E vanilla", "E2E paper" and "E2E neoforge" from loaders.e2e.mjs (skipped without them), and
// leaves them without the plugin and mod. Linux only. As root too, as app-killed.e2e.mjs says.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18757 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  await app?.stop();
});

const status = async (id) => (await client.call('GET', `/instances/${id}/commands`)).body;
const prepare = async (id) => (await client.call('POST', `/instances/${id}/commands/prepare`, {})).body;

async function complete(id, input) {
  const { status: code, body } = await client.call('GET', `/instances/${id}/commands/complete?${new URLSearchParams({ input })}`);
  assert.equal(code, 200, JSON.stringify(body));
  return body;
}

const texts = (completion) => completion.suggestions.map((suggestion) => suggestion.text);

async function server(t, name) {
  const found = await client.findInstance(name);
  if (!found?.installed) {
    t.skip(`no installed "${name}": run loaders.e2e.mjs first`);
    return null;
  }
  await removeContent(found.id);
  return found;
}

async function start(id) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
}

async function stop(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await until('the server to be down', async () => (await client.call('GET', `/instances/${id}`)).body.state.status === 'stopped', 180);
}

/** Prepares and waits until the generator has run and, if needed, `help` was learned. */
async function ready(id, { learned }) {
  const first = await prepare(id);
  if (first.vanilla.state === 'preparing') {
    const job = await client.waitForJob(first.vanilla.job_id, 900);
    assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-20).join('\n')}`);
  }
  if (learned) await prepare(id);
  return until('the commands ready', async () => {
    const now = await status(id);
    const done = now.vanilla.state === 'ready' && (!learned || now.learned.state === 'current');
    return done && now;
  }, 120);
}

async function installChunky(id) {
  const project = (await client.call('GET', `/instances/${id}/content/projects/chunky`)).body;
  const started = await client.call('POST', `/instances/${id}/content/install`, { project_id: project.id });
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const job = await client.waitForJob(started.body.job_id, 600);
  assert.equal(job.status, 'succeeded', job.error);
}

async function removeContent(id) {
  const items = (await client.call('GET', `/instances/${id}/content`)).body.items ?? [];
  for (const item of items) {
    await client.call('DELETE', `/instances/${id}/content/files/${encodeURIComponent(item.file)}`);
  }
}

/** What the server prints after `command`, sent through the console. */
async function answer(id, command, pattern) {
  const before = (await client.console(id)).length;
  await client.call('POST', `/instances/${id}/command`, { command });
  return until(`the answer to ${command}`, async () => (await client.console(id)).slice(before).find((line) => pattern.test(line)), 20);
}

test('vanilla 26.3: the generator’s commands, every one help lists, real ids, and a command the game takes', { skip, timeout: 20 * 60_000 }, async (t) => {
  const vanilla = await server(t, 'E2E vanilla');
  if (!vanilla) return;
  const id = vanilla.id;
  const done = await ready(id, { learned: false });
  assert.equal(done.vanilla.mc_version, '26.3');
  assert.ok(done.vanilla.commands >= 90, JSON.stringify(done));
  assert.equal(done.learned.state, 'not_needed');

  await start(id);
  try {
    // The game's own list, through its console.
    const before = (await client.console(id)).length;
    await client.call('POST', `/instances/${id}/command`, { command: 'help' });
    const listed = await until('help', async () => {
      const lines = (await client.console(id)).slice(before).filter((line) => /^(System chat: )?\/\S/.test(line));
      return lines.length > 80 && lines;
    }, 20);
    const names = listed.map((line) => line.replace(/^System chat: /, '').slice(1).split(' ')[0]);
    for (const name of names) {
      assert.ok(texts(await complete(id, name)).includes(name), `${name} completes`);
    }

    assert.ok(texts(await complete(id, 'give @p diamond_sw')).includes('diamond_sword'));
    assert.deepEqual(texts(await complete(id, 'enchant @p sharp')), ['sharpness'], 'an id from the game’s data pack');
    assert.ok(texts(await complete(id, 'locate biome minecraft:plai')).includes('minecraft:plains'));
    assert.ok(texts(await complete(id, 'effect give @a spe')).includes('speed'));

    // A completed command, sent as it was completed, the game takes.
    const rule = texts(await complete(id, 'gamerule keep_inv'));
    assert.ok(rule.includes('keep_inventory'), rule.join(', '));
    const reply = await answer(id, 'gamerule keep_inventory', /keep_inventory.*currently set to/i);
  } finally {
    await stop(id);
  }
});

test('Paper 26.3 with a plugin: its commands from Bukkit’s help, with what they do', { skip, timeout: 20 * 60_000 }, async (t) => {
  const paper = await server(t, 'E2E paper');
  if (!paper) return;
  const id = paper.id;
  await installChunky(id);
  try {
    await start(id);
    const done = await ready(id, { learned: true });
    assert.ok(done.learned_commands > 10, JSON.stringify(done));
    const chunky = (await complete(id, 'chunk')).suggestions.find((suggestion) => suggestion.text === 'chunky');
    assert.equal(chunky?.detail, 'Generates chunks');
    assert.ok(texts(await complete(id, 'vers')).includes('version'), 'Bukkit’s own commands');
    assert.ok(texts(await complete(id, 'minecraft:give @p dia')).includes('diamond'), 'namespaced vanilla commands lead to the vanilla ones');
    const console = await client.console(id);
    assert.ok(console.includes('[autocomplete] > help'), 'help was asked in the open');
  } finally {
    await stop(id);
    await removeContent(id);
  }
  assert.equal((await status(id)).learned.state, 'stale', 'other plugins: asked again next time');
});

test('NeoForge 26.3 with a mod: the loader’s and the mod’s commands from help', { skip, timeout: 20 * 60_000 }, async (t) => {
  const neoforge = await server(t, 'E2E neoforge');
  if (!neoforge) return;
  const id = neoforge.id;
  await installChunky(id);
  try {
    await start(id);
    await ready(id, { learned: true });
    assert.ok(texts(await complete(id, 'neofor')).includes('neoforge'));
    assert.deepEqual(texts(await complete(id, 'chunky st')), ['start']);
    // The mod's command as completed, the mod answers.
    // Not the echo of the command itself (`[admin] > chunky help`).
    const reply = await answer(id, 'chunky help', /^(?!\[admin\]).*chunky/i);
    assert.ok(reply, "Chunky answered");
  } finally {
    await stop(id);
    await removeContent(id);
  }
});
