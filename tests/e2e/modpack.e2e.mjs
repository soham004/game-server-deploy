// A real modpack from Modrinth becomes a real server: uploaded as an .mrpack, the loader and every
// file installed for real, started, then deleted again. It picks the smallest pack Modrinth has for
// the Minecraft version, and prints which.
//
//   node --test tests/e2e/modpack.e2e.mjs
//
// Linux only (the sandbox). Needs the internet. Environment: E2E_DATA (as loaders.e2e.mjs),
// E2E_MC (default 26.3).

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client } from './lib/client.mjs';
import { download, smallModpack } from './lib/small-modpack.mjs';

const MC = process.env.E2E_MC ?? '26.3';
const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18745 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  await app?.stop();
});

test('a real Fabric modpack installs, starts and loads its mods', { skip, timeout: 20 * 60_000 }, async (t) => {
  const pack = await smallModpack(MC, 'fabric');
  if (!pack) {
    t.skip(`Modrinth has no Fabric modpack for ${MC} that runs on a server`);
    return;
  }
  t.diagnostic(`using ${pack.title}: ${pack.filename} (${pack.size} bytes)`);

  const { status, body: uploaded } = await client.call('PUT', `/modpacks/upload?file=${encodeURIComponent(pack.filename)}`, await download(pack), { raw: true });
  assert.equal(status, 201, JSON.stringify(uploaded));
  assert.equal(uploaded.kind, 'pack');
  assert.equal(uploaded.pack.mc_version, MC);
  assert.equal(uploaded.pack.loader, 'fabric');
  t.diagnostic(`${uploaded.pack.files} files; loader ${uploaded.pack.loader_version}`);

  const started = await client.call('POST', '/modpacks/install', {
    upload_id: uploaded.upload_id,
    name: 'E2E modpack',
    max_memory_mb: 4096,
    eula_accepted: true,
  });
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const { id } = started.body.instance;
  try {
    const job = await client.waitForJob(started.body.job_id, 900);
    assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-20).join('\n')}`);

    const items = (await client.call('GET', `/instances/${id}/content`)).body.items;
    assert.ok(items.length > 0 || uploaded.pack.files === 0, 'its mods are listed');
    const tracked = items.filter((item) => item.source);
    t.diagnostic(`${items.length} mods, ${tracked.length} tracked`);
    assert.ok(items.length === 0 || tracked.length > 0, 'Modrinth recognises files from a Modrinth pack');

    await client.call('POST', `/instances/${id}/start`);
    await client.waitForState(id, 'running', 600).catch(async (error) => {
      throw new Error(`${error.message}\n${(await client.console(id)).slice(-40).join('\n')}`);
    });
    await client.call('POST', `/instances/${id}/stop`);
    await client.waitForState(id, 'stopped', 180);
  } finally {
    const deleted = await client.call('DELETE', `/instances/${id}?delete_backups=true`);
    assert.equal(deleted.status, 204, JSON.stringify(deleted.body));
  }
});
