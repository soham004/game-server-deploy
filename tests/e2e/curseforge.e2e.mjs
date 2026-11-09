// The real CurseForge API, with your own key, on real servers ("E2E fabric" and "E2E paper" from
// loaders.e2e.mjs): saving the key, search, a mod with its dependencies loading, identifying a file
// whose index entry is gone, updating, a Bukkit plugin enabling, and any download-blocked file seen.
//
//   node --test tests/e2e/curseforge.e2e.mjs
//
// The key comes from CURSEFORGE_API_KEY, or else from the `.env` file (lib/env.mjs). It is saved in
// the test data folder for the run, removed again at the end, and never printed. Skipped without
// one. Linux only (the sandbox). Which projects
// it uses depends on what CurseForge has for the servers' Minecraft version, so it picks them from
// search results and prints what it chose.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client } from './lib/client.mjs';
import { curseForgeKey } from './lib/env.mjs';

const KEY = curseForgeKey();
const skip = !KEY ? 'no CurseForge key: set CURSEFORGE_API_KEY, or put it in .env' : process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;

before(async () => {
  if (skip) return;
  // The app doesn't read this variable, and needn't inherit it: it gets the key the way a user gives it.
  delete process.env.CURSEFORGE_API_KEY;
  app = await startApp({ port: 18744 });
  client = new Client(app.origin);
  await client.signIn();
  // Every test needs it, whichever of them run.
  const saved = await client.call('PUT', '/settings/curseforge-key', { key: KEY });
  assert.equal(saved.status, 200, 'CurseForge accepts your key');
});

after(async () => {
  if (client) await client.call('DELETE', '/settings/curseforge-key');
  await app?.stop();
});

test('the key is checked and saved, and never comes back', { skip }, async () => {
  const wrong = await client.call('PUT', '/settings/curseforge-key', { key: 'not-a-real-key' });
  assert.equal(wrong.status, 422, 'CurseForge refuses a made-up key');
  assert.deepEqual((await client.call('GET', '/settings')).body, { curseforge_key_set: true }, 'and the good one stays');
  assert.ok(!JSON.stringify((await client.call('GET', '/settings')).body).includes(KEY));
  assert.ok(!app.log().includes(KEY), 'not in the app log');
});

test('Fabric: a mod with dependencies loads, is identified by fingerprint, and updates', { skip, timeout: 15 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E fabric');
  if (!server) return;
  const picked = await pick(server.id, 2);
  if (!picked) {
    t.skip('no mod on CurseForge with two downloadable files for this server');
    return;
  }
  const { hit, versions } = picked;
  t.diagnostic(`using ${hit.title} (${hit.id}): ${hit.page_url}`);

  const plan = await install(server.id, hit.id, versions[1].id);
  t.diagnostic(`plan: ${plan.steps.map((step) => step.project.title).join(', ')}`);
  let items = await listed(server.id);
  const installed = items.find((item) => item.source?.project_id === hit.id);
  assert.equal(installed?.source.source, 'curseforge', JSON.stringify(items));
  assert.equal(installed.sha1, versions[1].file.sha1, 'the download matches the SHA-1 CurseForge gives');

  // An update is offered for the older file; the update check is one request per project.
  const check = (await client.call('GET', `/instances/${server.id}/content/updates`)).body;
  const update = check.updates.find((candidate) => candidate.project_id === hit.id);
  assert.ok(update, `an update for ${hit.title}: ${JSON.stringify(check)}`);

  // Without its index entry the file is unknown; its fingerprint names it again.
  const entries = (await client.call('GET', `/instances/${server.id}/files?path=mods/.index`)).body.entries;
  const deleted = await client.call('POST', `/instances/${server.id}/files/delete`, { paths: entries.map((entry) => entry.path) });
  assert.equal(deleted.status, 204, JSON.stringify(deleted.body));
  const identified = (await client.call('POST', `/instances/${server.id}/content/identify`, {})).body;
  t.diagnostic(`identified: ${identified.identified.join(', ')}; unknown: ${identified.still_unknown.join(', ')}`);
  assert.ok(identified.identified.length > 0, JSON.stringify(identified));
  items = await listed(server.id);
  t.diagnostic(`sources after identifying: ${items.map((item) => `${item.name}=${item.source?.source}`).join(', ')}`);

  await runAndRead(server.id);
  await removeAll(server.id);
});

test('Paper: a Bukkit plugin from CurseForge installs and enables', { skip, timeout: 15 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E paper');
  if (!server) return;
  const picked = await pick(server.id, 1);
  if (!picked) {
    t.skip('no Bukkit plugin on CurseForge with a downloadable file for this server');
    return;
  }
  t.diagnostic(`using ${picked.hit.title} (${picked.hit.id}): ${picked.hit.page_url}`);
  await install(server.id, picked.hit.id, picked.versions[0].id);
  const lines = await runAndRead(server.id);
  t.diagnostic(`enabled: ${lines.filter((line) => line.includes('Enabling')).join(' | ').slice(0, 600)}`);
  await removeAll(server.id);
});

test('files whose authors block downloads say where to get them', { skip, timeout: 5 * 60_000 }, async (t) => {
  const server = await ready(t, 'E2E fabric');
  if (!server) return;
  for (const hit of await hits(server.id)) {
    const versions = (await client.call('GET', `/instances/${server.id}/content/projects/${hit.id}/versions?source=curseforge`)).body;
    const blocked = versions.find((version) => version.file.manual_url);
    if (blocked) {
      t.diagnostic(`${hit.title}: ${blocked.file.manual_url}`);
      assert.equal(blocked.file.url, '');
      return;
    }
  }
  t.diagnostic('no blocked file among the most downloaded projects: not checked against the real API');
});

test('a real modpack is found, downloaded, and made into a server', { skip, timeout: 30 * 60_000 }, async (t) => {
  const pack = await smallestPack();
  if (!pack) {
    t.skip('CurseForge has no modpack for the Minecraft versions tried');
    return;
  }
  t.diagnostic(`using ${pack.hit.title} (${pack.hit.id}), file ${pack.version.file.filename}: ${pack.hit.page_url}`);

  const started = await client.call('POST', '/modpacks/fetch', { source: 'curseforge', project_id: pack.hit.id, version_id: pack.version.id });
  if (started.status === 409) {
    assert.match(started.body.error.message, /curseforge\.com/, 'a pack only downloadable by hand says where to get it');
    t.diagnostic(`only by hand: ${started.body.error.message}`);
    return;
  }
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const fetched = await client.waitForJob(started.body.job_id, 900);
  t.diagnostic(`job: ${fetched.title}`);
  assert.equal(fetched.status, 'succeeded', fetched.error);
  const { body: uploaded } = await client.call('GET', `/modpacks/uploads/${started.body.upload_id}`);
  t.diagnostic(`it is: ${uploaded.kind} ${JSON.stringify(uploaded.pack)}, server pack: ${uploaded.server_pack}`);
  assert.equal(uploaded.kind, 'pack', JSON.stringify(uploaded));

  const created = await client.call('POST', '/modpacks/install', { upload_id: uploaded.upload_id, name: 'E2E CF pack', max_memory_mb: 4096, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const { id } = created.body.instance;
  try {
    const job = await client.waitForJob(created.body.job_id, 1200);
    t.diagnostic(`log: ${job.log.slice(-6).join(' | ').slice(0, 800)}`);
    assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-20).join('\n')}`);
    const { body: server } = await client.call('GET', `/instances/${id}`);
    assert.equal(server.modpack.source, 'curseforge');
    assert.equal(server.modpack.project_id, pack.hit.id);
    // A pack made for clients may not start on a server, which isn't this app's doing: only reported.
    await client.call('POST', `/instances/${id}/start`);
    const outcome = await client.waitForState(id, 'running', 420).then(() => 'it started', () => 'it did not start');
    const lines = await client.console(id);
    t.diagnostic(`${outcome}: ${lines.slice(-3).join(' | ').slice(0, 500)}`);
  } finally {
    await client.call('POST', `/instances/${id}/stop`);
    await client.waitForState(id, 'stopped', 180).catch(() => {});
    const deleted = await client.call('DELETE', `/instances/${id}?delete_backups=true`);
    assert.equal(deleted.status, 204, JSON.stringify(deleted.body));
  }
});

/** The pack whose newest file is smallest, among the most downloaded for a recent Minecraft version. */
async function smallestPack() {
  // A particular project, say one known to have a server pack.
  if (process.env.E2E_CF_PACK) {
    const versions = (await client.call('GET', `/modpacks/projects/${process.env.E2E_CF_PACK}/versions?source=curseforge`)).body;
    return { hit: { id: process.env.E2E_CF_PACK, title: 'E2E_CF_PACK', page_url: '' }, version: versions[0] };
  }
  for (const mc of ['26.3', '1.21.1', '1.20.1']) {
    const { status, body } = await client.call('GET', `/modpacks/search?source=curseforge&sort=downloads&mc=${mc}`);
    assert.equal(status, 200, JSON.stringify(body));
    const candidates = [];
    for (const hit of body.hits) {
      const versions = (await client.call('GET', `/modpacks/projects/${hit.id}/versions?source=curseforge`)).body;
      if (versions[0]) candidates.push({ hit, version: versions[0] });
    }
    candidates.sort((a, b) => a.version.file.size - b.version.file.size);
    if (candidates[0]) return candidates[0];
  }
  return null;
}

async function hits(id) {
  const { status, body } = await client.call('GET', `/instances/${id}/content/search?source=curseforge&sort=downloads`);
  assert.equal(status, 200, JSON.stringify(body));
  return body.hits;
}

/** The most downloaded project with `count` downloadable files, and those files. */
async function pick(id, count) {
  for (const hit of await hits(id)) {
    const { status, body } = await client.call('GET', `/instances/${id}/content/projects/${hit.id}/versions?source=curseforge`);
    assert.equal(status, 200, JSON.stringify(body));
    const versions = body.filter((version) => !version.file.manual_url);
    if (versions.length >= count) return { hit, versions };
  }
  return null;
}

async function ready(t, name) {
  const server = await client.findInstance(name);
  if (!server?.installed) {
    t.skip(`no installed "${name}": run loaders.e2e.mjs first`);
    return null;
  }
  await removeAll(server.id);
  return server;
}

async function listed(id) {
  return (await client.call('GET', `/instances/${id}/content`)).body.items;
}

async function install(id, projectId, versionId) {
  const started = await client.call('POST', `/instances/${id}/content/install`, { source: 'curseforge', project_id: projectId, version_id: versionId });
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const job = await client.waitForJob(started.body.job_id, 600);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.join('\n')}`);
  return started.body.plan;
}

async function runAndRead(id) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
  const lines = await client.console(id);
  await client.call('POST', `/instances/${id}/stop`);
  await client.waitForState(id, 'stopped', 180);
  return lines;
}

async function removeAll(id) {
  for (const item of await listed(id)) {
    const removed = await client.call('DELETE', `/instances/${id}/content/files/${encodeURIComponent(item.file)}`);
    assert.equal(removed.status, 204, JSON.stringify(removed.body));
  }
}
