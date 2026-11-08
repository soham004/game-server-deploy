// Real mods and plugins from the live Modrinth API on real servers ("E2E fabric" and "E2E paper"
// from loaders.e2e.mjs): install with dependencies, start the server, check the game loaded
// them, update an older version (after a backup), name a file whose index entry is gone, then
// remove them again.
//
//   node --test tests/e2e/loaders.e2e.mjs   # once, to have the servers
//   node --test tests/e2e/content.e2e.mjs
//
// Linux only (the sandbox). Needs the internet. Environment: E2E_DATA (as loaders.e2e.mjs).

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client } from './lib/client.mjs';

const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18743 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  await app?.stop();
});

test('Fabric: mods with their dependencies load in the real server', { skip, timeout: 15 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E fabric');
  if (!server) return;
  const lithium = await projectId(server.id, 'lithium');
  const appleskin = await projectId(server.id, 'appleskin');

  await install(server.id, lithium);
  const plan = await install(server.id, appleskin);
  assert.ok(plan.steps.some((step) => step.project.slug === 'fabric-api'), 'AppleSkin brings Fabric API');
  const items = (await client.call('GET', `/instances/${server.id}/content`)).body.items;
  assert.equal(items.length, 3, items.map((item) => item.name).join(', '));
  assert.ok(items.every((item) => item.source?.source === 'modrinth'), 'all recorded in the index');

  const lines = await runAndRead(server.id);
  for (const mod of ['lithium', 'appleskin', 'fabric-api']) {
    assert.ok(lines.some((line) => line.includes(mod)), `the server loaded ${mod}`);
  }
  await removeAll(server.id);
});

test('Fabric: an older mod updates after a backup, and an untracked jar is identified', { skip, timeout: 15 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E fabric');
  if (!server) return;
  const lithium = await projectId(server.id, 'lithium');
  const versions = (await client.call('GET', `/instances/${server.id}/content/projects/${lithium}/versions`)).body;
  assert.ok(versions.length >= 2, `an older Lithium for this server: ${versions.length}`);
  await install(server.id, lithium, versions[1].id);

  // Fabric API without its index entry: as if copied in by hand.
  await install(server.id, await projectId(server.id, 'fabric-api'));
  const deleted = await client.call('POST', `/instances/${server.id}/files/delete`, { paths: ['mods/.index/fabric-api.pw.toml'] });
  assert.equal(deleted.status, 204, JSON.stringify(deleted.body));

  const check = (await client.call('GET', `/instances/${server.id}/content/updates`)).body;
  assert.deepEqual(
    check.updates.map((update) => [update.name, update.installed_version, update.version.id]),
    [['Lithium', versions[1].version_number, versions[0].id]],
    'the installed version as Modrinth numbers it',
  );
  assert.equal(check.unknown.length, 1, JSON.stringify(check));
  const identified = (await client.call('POST', `/instances/${server.id}/content/identify`, {})).body;
  assert.deepEqual(identified.identified, ['Fabric API'], JSON.stringify(identified));

  const started = await client.call('POST', `/instances/${server.id}/content/update`, {});
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const job = await client.waitForJob(started.body.job_id, 600);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.join('\n')}`);
  const items = (await client.call('GET', `/instances/${server.id}/content`)).body.items;
  const updated = items.find((item) => item.source?.project_id === lithium);
  assert.equal(updated.source.version_id, versions[0].id, 'the newest Lithium');
  assert.equal(items.length, 2, items.map((item) => item.file).join(', '));
  const backups = (await client.call('GET', `/instances/${server.id}/backups`)).body.backups;
  const before = backups.find((backup) => backup.kind === 'pre_update');
  assert.equal(before?.scope, 'full', 'a full backup before updating');
  await client.call('DELETE', `/instances/${server.id}/backups/${before.id}`);

  const lines = await runAndRead(server.id);
  assert.ok(lines.some((line) => line.includes('lithium')), 'the server loaded the updated Lithium');
  await removeAll(server.id);
});

test('Paper: a plugin from Modrinth enables in the real server', { skip, timeout: 15 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E paper');
  if (!server) return;
  await install(server.id, await projectId(server.id, 'chunky'));
  const lines = await runAndRead(server.id);
  assert.ok(lines.some((line) => /Enabling Chunky/.test(line)), 'Chunky enabled');
  await removeAll(server.id);
});

/** The installed, stopped server named `name`, or a skip when loaders.e2e.mjs hasn't run. */
async function ready(t, name) {
  const server = await client.findInstance(name);
  if (!server?.installed) {
    t.skip(`no installed "${name}": run loaders.e2e.mjs first`);
    return null;
  }
  await removeAll(server.id);
  return server;
}

async function projectId(id, slug) {
  const { status, body } = await client.call('GET', `/instances/${id}/content/projects/${slug}`);
  assert.equal(status, 200, JSON.stringify(body));
  return body.id;
}

async function install(id, projectId, versionId = undefined) {
  const started = await client.call('POST', `/instances/${id}/content/install`, { project_id: projectId, version_id: versionId });
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const job = await client.waitForJob(started.body.job_id, 600);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.join('\n')}`);
  return started.body.plan;
}

/** Starts the server, waits until it runs, returns its console, stops it. */
async function runAndRead(id) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
  const lines = await client.console(id);
  const { diagnostics } = (await client.call('GET', `/instances/${id}/console`)).body;
  assert.deepEqual(diagnostics, [], 'no problems detected');
  await client.call('POST', `/instances/${id}/stop`);
  await client.waitForState(id, 'stopped', 180);
  return lines;
}

async function removeAll(id) {
  for (const item of (await client.call('GET', `/instances/${id}/content`)).body.items) {
    const removed = await client.call('DELETE', `/instances/${id}/content/files/${encodeURIComponent(item.file)}`);
    assert.equal(removed.status, 204, JSON.stringify(removed.body));
  }
}
