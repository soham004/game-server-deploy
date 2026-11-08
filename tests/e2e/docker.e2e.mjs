// The Docker image as the compose file runs it (root, the minimal capabilities, Docker's default
// read-only cgroupfs): hard limits report themselves off with a reason, and every file the app
// writes into a server (install, upload, folder, extract, clone, import, restore, version
// change) belongs to that server's own user, never root. Backups are root-only, a running world
// is paused and flushed for one, and a server deleted with its backups leaves both in the trash.
//
//   docker build -t gsd:dev . && node --test tests/e2e/docker.e2e.mjs
//
// Needs Docker and the internet (real Minecraft downloads). Environment: E2E_IMAGE (default
// gsd:dev); E2E_REQUIRED=1 fails instead of skipping when the image is missing (CI runs it so).

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { after, before, test } from 'node:test';

import { Client, until } from './lib/client.mjs';

const IMAGE = process.env.E2E_IMAGE ?? 'gsd:dev';
const CONTAINER = 'gsd-docker-e2e';
const PORT = 18741;
const CAPABILITIES = ['SETUID', 'SETGID', 'CHOWN', 'DAC_OVERRIDE', 'FOWNER', 'KILL'];
// Skips without the image, unless E2E_REQUIRED is set (CI): then a missing image fails the run.
const hasImage = spawnSync('docker', ['image', 'inspect', IMAGE]).status === 0;
const skip = hasImage || process.env.E2E_REQUIRED ? false : `no Docker image ${IMAGE}`;

const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8' }).trim();
/** Runs a shell command in the container; returns its output. */
const sh = (script) => docker('exec', CONTAINER, 'sh', '-c', script);
/** The container's output (both streams), without colour codes. */
const logs = () => {
  const result = spawnSync('docker', ['logs', CONTAINER], { encoding: 'utf8' });
  return (result.stdout + result.stderr).replace(/\x1b\[[0-9;]*m/g, '');
};

const client = new Client(`http://127.0.0.1:${PORT}`);

before(async () => {
  if (skip) return;
  spawnSync('docker', ['rm', '-f', CONTAINER]);
  // The port the app listens on inside the image is a build-time brand setting: ask the image.
  const appPort = docker('run', '--rm', IMAGE, 'brand', 'port');
  docker('run', '-d', '--name', CONTAINER, '--cap-drop', 'ALL', ...CAPABILITIES.flatMap((cap) => ['--cap-add', cap]),
    '-p', `127.0.0.1:${PORT}:${appPort}`, IMAGE);
  await until('the app', async () => (await client.call('GET', '/health')).status === 200, 60);
  const token = await until('the setup token in the log', async () => logs().match(/setup token to create the owner: (\S+)/)?.[1], 30);
  await client.signIn({ setupToken: token });
});

after(() => {
  if (skip) return;
  const problems = logs().split('\n').filter((line) => /WARN|ERROR/.test(line) && !line.includes('setup token'));
  if (problems.length) console.log(`--- app warnings and errors\n${problems.slice(0, 15).join('\n')}`);
  spawnSync('docker', ['rm', '-f', CONTAINER]);
});

test('the image, as root', { skip, timeout: 20 * 60_000 }, async (t) => {
  let id;
  let server;
  let uid;
  let archived;
  let backup;

  await t.test('hard limits are off, with the reason', async () => {
    const { body: system } = await client.call('GET', '/system');
    assert.equal(system.hard_limits, false, 'Docker mounts cgroupfs read-only by default');
    assert.match(system.hard_limits_problem, /Read-only file system.*Docker/);
    assert.match(logs(), /hard memory limits are off/);
  });

  await t.test('an installed server belongs to its own user', async () => {
    id = await create('Owned');
    server = sh('ls -d /data/instances/owned/server');
    uid = sh(`stat -c %u ${server}`);
    assert.notEqual(uid, '0');
    assertOwnedBy(server, uid);
  });

  await t.test('uploads, folders, archives and extracted files belong to it too', async () => {
    const files = `/instances/${id}/files`;
    assert.equal((await client.call('POST', `${files}/folder`, { path: 'pack' })).status, 201);
    assert.ok((await client.call('PUT', `${files}/upload?path=pack/notes.txt`, 'hello', { raw: true })).status < 300);
    assert.ok((await client.call('POST', `${files}/compress`, { paths: ['pack'], destination: 'pack.zip' })).status < 300);
    assert.ok((await client.call('POST', `${files}/extract`, { path: 'pack.zip', destination: 'unpacked' })).status < 300);
    assert.equal(sh(`cat ${server}/unpacked/pack/notes.txt`), 'hello');
    assertOwnedBy(server, uid);
  });

  await t.test('a clone gets a user of its own', async () => {
    const clone = await client.call('POST', `/instances/${id}/clone`, { name: 'Owned copy', include_worlds: false });
    assert.equal(clone.status, 202, JSON.stringify(clone.body));
    await succeeded(clone.body.job_id);
    const folder = sh('ls -d /data/instances/owned-copy/server');
    const cloneUid = sh(`stat -c %u ${folder}`);
    assert.ok(![uid, '0'].includes(cloneUid), `clone uid ${cloneUid}`);
    assertOwnedBy(folder, cloneUid);
  });

  await t.test('an import is copied for its own user; the original stays root\'s', async () => {
    sh(`mkdir -p /data/imports/legacy/config && cp ${server}/server.jar /data/imports/legacy/server.jar && echo motd=hi > /data/imports/legacy/server.properties && echo x > /data/imports/legacy/config/a.cfg`);
    const imported = await client.call('POST', '/imports', {
      folder: 'legacy', name: 'Imported', mc_version: '26.3', loader: 'vanilla', jar: 'server.jar', max_memory_mb: 1024, eula_accepted: true,
    });
    assert.equal(imported.status, 202, JSON.stringify(imported.body));
    await succeeded(imported.body.job_id);
    const folder = sh('ls -d /data/instances/imported/server');
    const importUid = sh(`stat -c %u ${folder}`);
    assert.ok(![uid, '0'].includes(importUid), `import uid ${importUid}`);
    assertOwnedBy(folder, importUid);
    assert.equal(sh('stat -c %u /data/imports/legacy/server.jar'), '0');
  });

  await t.test('a running server is paused and flushed for a root-only backup', async () => {
    await client.call('POST', `/instances/${id}/start`);
    await client.waitForState(id, 'running', 240);
    const started = await client.call('POST', `/instances/${id}/backups`, { scope: 'world', note: 'docker' });
    assert.equal(started.status, 202, JSON.stringify(started.body));
    await succeeded(started.body.job_id);
    const lines = (await client.console(id)).join('\n');
    for (const expected of ['[backup] > save-off', 'Saved the game', '[backup] > save-on']) {
      assert.ok(lines.includes(expected), `the console shows ${expected}`);
    }
    [backup] = (await client.call('GET', `/instances/${id}/backups`)).body.backups;
    const download = await fetch(`${client.api}/instances/${id}/backups/${backup.id}/download`, { headers: { Cookie: client.cookie } });
    assert.equal(download.status, 200);
    archived = zipEntries(Buffer.from(await download.arrayBuffer()))
      .filter((name) => name.startsWith('server/world/') && !name.endsWith('/'))
      .map((name) => name.slice('server/'.length));
    assert.ok(archived.includes('world/level.dat'), archived.join('\n'));
    assert.ok(archived.some((name) => name.endsWith('.mca')), 'region files are in it');
    assert.equal(sh(`stat -c '%a %U' /data/backups/${id}`), '700 root');
    assert.equal(sh(`stat -c %U /data/backups/${id}/${backup.file_name}`), 'root');
  });

  await t.test('a restore brings the world back for the server\'s user and restarts it', async () => {
    sh(`echo after > ${server}/world/after-backup.txt && chown ${uid}:${uid} ${server}/world/after-backup.txt`);
    const started = await client.call('POST', `/instances/${id}/backups/${backup.id}/restore`, {});
    assert.equal(started.status, 202, JSON.stringify(started.body));
    await succeeded(started.body.job_id);
    assert.equal(sh(`test -e ${server}/world/after-backup.txt && echo present || echo gone`), 'gone');
    const present = sh(`cd ${server} && find world -type f`).split('\n');
    assert.deepEqual(archived.filter((name) => !present.includes(name)), [], 'every archived file is back');
    assertOwnedBy(server, uid);
    assert.deepEqual(sh('ls /data/instances/owned').split('\n').filter((name) => name.startsWith('restore')), [], 'no staging folders left');
    await client.waitForState(id, 'running', 240);
    const kinds = (await client.call('GET', `/instances/${id}/backups`)).body.backups.map((b) => b.kind).sort();
    assert.deepEqual(kinds, ['manual', 'pre_restore']);
    await client.call('POST', `/instances/${id}/stop`);
    await client.waitForState(id, 'stopped', 120);
  });

  await t.test('a version change leaves every file with the server\'s user', async () => {
    const changed = await client.call('POST', `/instances/${id}/change-version`, { mc_version: '26.2', allow_downgrade: true });
    assert.equal(changed.status, 202, JSON.stringify(changed.body));
    await succeeded(changed.body.job_id);
    assert.equal((await client.call('GET', `/instances/${id}`)).body.software, 'Vanilla 26.2');
    assertOwnedBy(server, uid);
  });

  await t.test('deleting it with its backups puts both in the trash', async () => {
    assert.equal((await client.call('DELETE', `/instances/${id}?delete_backups=true`)).status, 204);
    const trashed = sh('ls /data/trash').split('\n');
    assert.ok(trashed.some((name) => /^\d{8}-\d{6}-owned$/.test(name)), trashed.join(', '));
    assert.ok(trashed.some((name) => /^\d{8}-\d{6}-owned-backups$/.test(name)), trashed.join(', '));
    assert.equal(sh(`test -e /data/backups/${id} && echo present || echo gone`), 'gone');
  });
});

async function create(name) {
  const created = await client.call('POST', '/instances', { name, mc_version: '26.3', loader: 'vanilla', max_memory_mb: 1024, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  await succeeded(created.body.job_id);
  return created.body.instance.id;
}

async function succeeded(jobId) {
  const job = await client.waitForJob(jobId, 600);
  assert.equal(job.status, 'succeeded', `${job.error}\n${job.log.slice(-10).join('\n')}`);
}

/** Every entry under `dir` in the container belongs to `uid`. */
function assertOwnedBy(dir, uid) {
  const wrong = sh(`find ${dir} ! -uid ${uid}`);
  assert.equal(wrong, '', `everything under ${dir} belongs to uid ${uid}; these don't:\n${wrong}`);
}

/** Entry names of a zip, from its central directory (no zip64: these archives are small). */
function zipEntries(buffer) {
  let end = buffer.length - 22;
  while (end >= 0 && buffer.readUInt32LE(end) !== 0x06054b50) end -= 1;
  assert.ok(end >= 0, 'not a zip');
  const count = buffer.readUInt16LE(end + 10);
  let offset = buffer.readUInt32LE(end + 16);
  const names = [];
  for (let i = 0; i < count; i += 1) {
    assert.equal(buffer.readUInt32LE(offset), 0x02014b50, 'central directory entry');
    const nameLength = buffer.readUInt16LE(offset + 28);
    const extraLength = buffer.readUInt16LE(offset + 30);
    const commentLength = buffer.readUInt16LE(offset + 32);
    names.push(buffer.toString('utf8', offset + 46, offset + 46 + nameLength));
    offset += 46 + nameLength + extraLength + commentLength;
  }
  return names;
}
