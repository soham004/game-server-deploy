// Worlds on a real 26.3 server, checked with the game's own answers: the seed the app reads from the
// files is the one `seed` prints; a world downloaded and uploaded again under another name, then made
// active, is the same world; a reset with a text seed gives a new world with that seed; deleting another
// world backs it up first; an active world the game can't read is deleted, and the next start makes a new
// one. A Paper server's nether and end are never worlds of their own.
//
//   node --test tests/e2e/worlds.e2e.mjs
//
// Makes its own server and deletes it again; reads "E2E paper" from loaders.e2e.mjs, if there is one.
// Linux only. As root too, as app-killed.e2e.mjs says.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
const NAME = 'E2E worlds';
let app;
let client;
let id;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18755 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  if (id) await remove(id).catch(() => {});
  await app?.stop();
});

const down = (server) => until('the server to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${server}`)).body.state.status), 180);

async function remove(server) {
  await client.call('POST', `/instances/${server}/stop`);
  await down(server);
  await client.call('DELETE', `/instances/${server}?delete_backups=true`);
}

async function start() {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
}

async function stop() {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
}

const worlds = async () => (await client.call('GET', `/instances/${id}/worlds`)).body;

/** The seed the running game says it has. */
async function gameSeed() {
  const before = (await client.console(id)).length;
  await client.call('POST', `/instances/${id}/command`, { command: 'seed' });
  return until('the game to say its seed', async () => {
    const line = (await client.console(id)).slice(before).find((text) => /Seed: \[/.test(text));
    return line?.match(/Seed: \[(-?\d+)\]/)?.[1];
  }, 15);
}

/** Java's `String.hashCode`, which the game uses for a seed that isn't a number. */
function javaHash(text) {
  let hash = 0;
  for (const char of text) hash = (Math.imul(hash, 31) + char.charCodeAt(0)) | 0;
  return String(hash);
}

async function job(response) {
  assert.equal(response.status, 202, JSON.stringify(response.body));
  return client.waitForJob(response.body.job_id, 900);
}

test('a real world: its facts, download and upload, reset with a seed, delete after a backup', { skip, timeout: 25 * 60_000 }, async (t) => {
  const leftover = await client.findInstance(NAME);
  if (leftover) await remove(leftover.id);
  const created = await client.call('POST', '/instances', { name: NAME, mc_version: process.env.E2E_MC ?? '26.3', loader: 'vanilla', max_memory_mb: 1536, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  id = created.body.instance.id;
  assert.equal((await client.waitForJob(created.body.job_id, 900)).status, 'succeeded');
  assert.deepEqual((await worlds()).worlds, [], 'no world before the first start');

  await start();
  const seed = await gameSeed();
  const running = await worlds();
  assert.equal(running.running, true);
  assert.equal(running.worlds[0].seed, seed, 'the seed in the files is the one the game has');
  const busy = await client.call('GET', `/instances/${id}/worlds/world/download`);
  assert.equal(busy.status, 409, 'not while it runs');
  await stop();

  const world = (await worlds()).worlds[0];
  t.diagnostic(JSON.stringify({ ...world, datapacks: world.datapacks.enabled }));
  assert.equal(world.folder, 'world');
  assert.equal(world.version, process.env.E2E_MC ?? '26.3');
  assert.equal(world.game_mode, 'survival');
  assert.ok(world.size_bytes > 100_000, `${world.size_bytes} bytes`);
  assert.ok(world.datapacks.enabled.includes('vanilla'));

  // Downloaded, and uploaded again as another world, which is then played.
  const zip = await fetch(`${app.origin}/api/v1/instances/${id}/worlds/world/download`, { headers: { Cookie: client.cookie } });
  assert.equal(zip.status, 200);
  const bytes = Buffer.from(await zip.arrayBuffer());
  assert.ok(bytes.includes(Buffer.from('world/level.dat')), 'level.dat is in the zip');
  const uploaded = await client.call('PUT', `/instances/${id}/worlds/copy?activate=true`, bytes, { raw: true });
  assert.equal(uploaded.status, 201, JSON.stringify(uploaded.body));
  assert.equal(uploaded.body.seed, seed);
  await start();
  assert.equal(await gameSeed(), seed, 'the copy is the same world');
  assert.ok((await client.console(id)).some((line) => line.includes('"copy"')), 'and the game opened the copy');
  await stop();

  // Reset with a text seed: the next start makes a new world with it.
  const reset = await job(await client.call('POST', `/instances/${id}/worlds/copy/reset`, { seed: 'glacier' }));
  assert.equal(reset.status, 'succeeded', reset.error);
  assert.ok(!(await worlds()).worlds.some((w) => w.folder === 'copy'));
  await start();
  assert.equal(await gameSeed(), javaHash('glacier'), 'the new world has the seed asked for');
  await stop();

  // The original isn't played any more: delete it, after a backup.
  const deleted = await job(await client.call('DELETE', `/instances/${id}/worlds/world`));
  assert.equal(deleted.status, 'succeeded', deleted.error);
  assert.deepEqual((await worlds()).worlds.map((w) => w.folder), ['copy']);
  const backups = (await client.call('GET', `/instances/${id}/backups`)).body.backups;
  assert.deepEqual(backups.map((b) => [b.kind, b.scope]), [['pre_delete', 'full'], ['pre_delete', 'world']]);

  // A world that won't load: its level.dat and the copy the game keeps of it are garbage. The game won't
  // play it, the app says why, and deleting it (the active one) lets the next start make a new world.
  const files = ['copy/level.dat', 'copy/level.dat_old'];
  const trashed = await client.call('POST', `/instances/${id}/files/delete`, { paths: files });
  assert.ok(trashed.status < 300, JSON.stringify(trashed.body));
  for (const file of files) {
    const written = await client.call('PUT', `/instances/${id}/files/text`, { path: file, content: 'not a level' });
    assert.ok(written.status < 300, JSON.stringify(written.body));
  }
  const broken = (await worlds()).worlds.find((w) => w.folder === 'copy');
  assert.ok(broken?.active && broken.problem?.startsWith("its level.dat can't be read"), JSON.stringify(broken));
  const linesBefore = (await client.console(id)).length;
  await client.call('POST', `/instances/${id}/start`);
  // "stopped" counts only once the game has printed something: before that, the start hasn't begun.
  const outcome = await until('the game to give up on the world, or play it', async () => {
    const { status } = (await client.call('GET', `/instances/${id}`)).body.state;
    const printed = (await client.console(id)).length > linesBefore;
    return status === 'running' || status === 'crashed' || (status === 'stopped' && printed) ? status : undefined;
  }, 300);
  const log = (await client.console(id)).slice(-15).join('\n');
  t.diagnostic(`broken world: ${outcome}\n${log}`);
  assert.notEqual(outcome, 'running', 'the game doesn’t play a world it can’t read');
  await stop();
  const gone = await job(await client.call('DELETE', `/instances/${id}/worlds/copy`));
  assert.equal(gone.status, 'succeeded', gone.error);
  assert.deepEqual((await worlds()).worlds, [], 'nothing left');
  await start();
  const fresh = await worlds();
  assert.deepEqual(fresh.worlds.map((w) => [w.folder, w.problem]), [['copy', null]], 'a new world under the same name');
  await stop();
});

test('a real Paper server’s world is one world, its nether and end with it', { skip, timeout: 60_000 }, async (t) => {
  const paper = await client.findInstance('E2E paper');
  if (!paper) {
    t.skip('no "E2E paper": run loaders.e2e.mjs first');
    return;
  }
  const view = (await client.call('GET', `/instances/${paper.id}/worlds`)).body;
  const main = view.worlds.find((w) => w.folder === view.active);
  assert.ok(main, JSON.stringify(view));
  // Paper 26.3 keeps its dimensions inside the world folder, as vanilla does; older Paper made
  // `world_nether` and `world_the_end`, which then show as the world's parts (the API tests cover those).
  t.diagnostic(`parts: ${JSON.stringify(main.parts)}`);
  assert.ok(!view.worlds.some((w) => w.folder.endsWith('_nether') || w.folder.endsWith('_the_end')), 'never listed as worlds of their own');
});
