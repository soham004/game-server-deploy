// The control channel on real servers: every loader on 26.3 connects to its management protocol, which
// listens on loopback only, and an old server (1.20.1) gets RCON, which is checked by logging in with the real
// password and running a command. It prints what each address really is, and the 26.3 protocol's methods.
//
//   node --test tests/e2e/control.e2e.mjs
//
// Uses the servers from loaders.e2e.mjs, and makes (and deletes) its own old one. Linux only. Environment:
// E2E_DATA, as loaders.e2e.mjs; E2E_LOADERS limits which loaders are tried. As root too, as app-killed.e2e.mjs says.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';

import { startApp } from './lib/app.mjs';
import { Client, until } from './lib/client.mjs';

const LOADERS = (process.env.E2E_LOADERS ?? 'vanilla paper purpur fabric quilt forge neoforge').split(/\s+/);
const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
let app;
let client;

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18750 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  await app?.stop();
});

/** The addresses something listens on for this TCP port, from the kernel's own table. */
function listeningOn(port) {
  const hex = port.toString(16).toUpperCase().padStart(4, '0');
  const addresses = [];
  for (const [file, label] of [['/proc/net/tcp', 4], ['/proc/net/tcp6', 6]]) {
    for (const line of readFileSync(file, 'utf8').split('\n').slice(1)) {
      const [, local, , state] = line.trim().split(/\s+/);
      if (state !== '0A' || !local?.endsWith(`:${hex}`)) continue;
      addresses.push(`${label === 4 ? 'v4' : 'v6'}:${local.split(':')[0]}`);
    }
  }
  return addresses;
}

const down = (id) => until('the server to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${id}`)).body.state.status), 180);

async function stop(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
}

async function runningWithControl(id, name) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${name}: ${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
  // A server is "running" a moment before everything it starts is listening.
  return until(`${name} to answer on its control channel`, async () => {
    const { body } = await client.call('GET', `/instances/${id}/control`);
    return body.connected ? body : null;
  }, 60).catch(async (error) => {
    const { body } = await client.call('GET', `/instances/${id}/control`);
    throw new Error(`${error.message}: ${JSON.stringify(body)}\n${(await client.console(id)).slice(-20).join('\n')}`);
  });
}

async function controlPort(id) {
  const { body } = await client.call('GET', `/instances/${id}/files/text?path=server.properties`);
  const line = (key) => body.content.split('\n').find((candidate) => candidate.startsWith(`${key}=`))?.split('=')[1];
  return { management: Number(line('management-server-port')), rcon: Number(line('rcon.port')), enabled: line('enable-rcon') };
}

for (const loader of LOADERS) {
  test(`${loader} 26.3: the management protocol answers, on loopback only`, { skip, timeout: 15 * 60_000 }, async (t) => {
    const server = await client.findInstance(`E2E ${loader}`);
    if (!server?.installed) {
      t.skip(`no installed "E2E ${loader}": run loaders.e2e.mjs first`);
      return;
    }
    try {
      const status = await runningWithControl(server.id, loader);
      assert.equal(status.channel, 'management');
      assert.ok(status.methods.includes('minecraft:players'), JSON.stringify(status.methods));
      const { management } = await controlPort(server.id);
      const addresses = listeningOn(management);
      t.diagnostic(`API ${status.api_version}, ${status.methods.length} methods, port ${management} on ${addresses.join(', ')}`);
      // 127.0.0.1, which Java can also open as an IPv4 address written in IPv6 (::ffff:127.0.0.1).
      assert.ok(addresses.length > 0 && addresses.every((address) => ['v4:0100007F', 'v6:0000000000000000FFFF00000100007F'].includes(address)), `loopback only: ${addresses}`);
      if (loader === 'vanilla') t.diagnostic(`methods: ${status.methods.join(' ')}`);
    } finally {
      await stop(server.id);
    }
  });
}

test('an old server keeps RCON off until asked; then it gets a real password that really works', { skip, timeout: 20 * 60_000 }, async (t) => {
  const created = await client.call('POST', '/instances', { name: 'E2E old rcon', mc_version: '1.20.1', loader: 'vanilla', max_memory_mb: 1024, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const { id } = created.body.instance;
  try {
    const job = await client.waitForJob(created.body.job_id, 900);
    assert.equal(job.status, 'succeeded', job.error);
    // Off by default: it would listen on every network interface.
    await client.call('POST', `/instances/${id}/start`);
    await client.waitForState(id, 'running', 420);
    const off = (await client.call('GET', `/instances/${id}/control`)).body;
    assert.equal(off.channel, 'rcon');
    assert.equal(off.connected, false, JSON.stringify(off));
    assert.equal(off.problem, null, 'off is not a problem');
    assert.notEqual((await controlPort(id)).enabled, 'true');
    await stop(id);

    // Turned on by the user, in Properties.
    const { body: file } = await client.call('GET', `/instances/${id}/files/text?path=server.properties`);
    const content = `${file.content.split('\n').filter((line) => !line.startsWith('enable-rcon=')).join('\n')}\nenable-rcon=true\n`;
    const saved = await client.call('PUT', `/instances/${id}/files/text`, { path: 'server.properties', revision: file.revision, content });
    assert.equal(saved.status, 200, JSON.stringify(saved.body));

    const status = await runningWithControl(id, 'the old server');
    assert.equal(status.channel, 'rcon');
    const { rcon } = await controlPort(id);
    t.diagnostic(`RCON on port ${rcon}, listening on ${listeningOn(rcon).join(', ')}`);
    assert.ok(listeningOn(rcon).length > 0);
  } finally {
    await stop(id);
    const deleted = await client.call('DELETE', `/instances/${id}?delete_backups=true`);
    assert.equal(deleted.status, 204, JSON.stringify(deleted.body));
  }
});
