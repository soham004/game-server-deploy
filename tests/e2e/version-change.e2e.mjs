// A real version change: the server built by loaders.e2e.mjs ("E2E neoforge" by default) moves to
// another build of its loader for the same Minecraft version, through the real installer, and
// starts; then back. Checks the safety backup, that the old build's own folder is gone, and that
// the server runs on the new build.
//
//   node --test tests/e2e/loaders.e2e.mjs   # once, to have the server
//   node --test tests/e2e/version-change.e2e.mjs
//
// Environment: E2E_DATA (as loaders.e2e.mjs), E2E_LOADER (default neoforge; forge works too).

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client } from './lib/client.mjs';

const LOADER = process.env.E2E_LOADER ?? 'neoforge';
const NAME = `E2E ${LOADER}`;
/** Where each build keeps its files (the folder that must go when the build changes). */
const BUILDS_FOLDER = {
  neoforge: 'libraries/net/neoforged/neoforge',
  forge: 'libraries/net/minecraftforge/forge',
}[LOADER];
const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';

let app;
let client;
let server;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18742 });
  client = new Client(app.origin);
  await client.signIn();
  server = await client.findInstance(NAME);
});

after(async () => {
  await app?.stop();
});

test(`${NAME} changes build and back`, { skip, timeout: 40 * 60_000 }, async (t) => {
  if (!server?.installed) {
    t.skip(`no installed "${NAME}": run loaders.e2e.mjs first`);
    return;
  }
  const mc = server.server.mc_version;
  const original = server.server.loader_version;
  const builds = (await client.call('GET', `/catalog/minecraft/loaders/${LOADER}/versions?mc=${mc}`)).body.map((v) => v.id);
  const other = builds.find((build) => build !== original);
  assert.ok(other, `another ${LOADER} build for ${mc} (have: ${builds.join(', ')})`);
  if (BUILDS_FOLDER) await assertOnlyBuild(original);

  await changeTo(mc, other);
  await runOnce(other);
  await changeTo(mc, original);
  await runOnce(original);

  // The safety backups were only for this test.
  for (const backup of (await client.call('GET', `/instances/${server.id}/backups`)).body.backups) {
    if (backup.kind === 'pre_version_change') await client.call('DELETE', `/instances/${server.id}/backups/${backup.id}`);
  }
});

async function changeTo(mc, build) {
  const started = await client.call('POST', `/instances/${server.id}/change-version`, { mc_version: mc, loader_version: build });
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const job = await client.waitForJob(started.body.job_id, 30 * 60);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-15).join('\n')}`);
  assert.ok(job.log.some((line) => line.includes(`Now on`) && line.includes(build)), job.log.join('\n'));

  assert.equal((await client.call('GET', `/instances/${server.id}`)).body.server.loader_version, build);
  const [safety] = (await client.call('GET', `/instances/${server.id}/backups`)).body.backups;
  assert.deepEqual([safety.kind, safety.scope], ['pre_version_change', 'full'], 'a full backup was taken first');
  if (BUILDS_FOLDER) await assertOnlyBuild(build);
}

async function runOnce(build) {
  await client.call('POST', `/instances/${server.id}/start`);
  await client.waitForState(server.id, 'running', 420);
  assert.ok((await client.console(server.id)).some((line) => line.includes(build)), `the console mentions ${build}`);
  await client.call('POST', `/instances/${server.id}/stop`);
  await client.waitForState(server.id, 'stopped', 180);
}

/** Exactly one build folder is installed, and it is `build`'s (Forge names it "26.3-66.1.0"). */
async function assertOnlyBuild(build) {
  const { body } = await client.call('GET', `/instances/${server.id}/files?path=${BUILDS_FOLDER}`);
  const folders = body.entries.map((entry) => entry.name);
  assert.equal(folders.length, 1, `one build folder: ${folders.join(', ')}`);
  assert.ok(folders[0] === build || folders[0].endsWith(`-${build}`), `${folders[0]} is ${build}`);
}
