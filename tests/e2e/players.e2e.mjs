// Players on real servers, with a real player: `lib/bot.mjs` joins in offline mode, so joins, leaves, kicks
// and bans are the game's own. A 26.3 server is changed through its management protocol while it runs and
// through its files while it is stopped; a 1.20.1 server through its console. Mojang's real API gives an
// account's UUID and head.
//
//   node --test tests/e2e/players.e2e.mjs
//
// Makes its own servers (offline mode) and deletes them again. Linux only. As root too, as app-killed.e2e.mjs
// says. Needs the internet (server downloads, Mojang).

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { inflateSync } from 'node:zlib';

import { startApp } from './lib/app.mjs';
import { joinServer, offlineUuid } from './lib/bot.mjs';
import { Client, until } from './lib/client.mjs';

const skip = process.platform === 'linux' ? false : 'real servers need Linux (the sandbox)';
const BOT = 'E2E_Bot';
const NOTCH = '069a79f4-44e9-4726-a5be-fca90e38aaf5';
let app;
let client;
const made = [];

before(async () => {
  if (skip) return;
  app = await startApp({ port: 18752 });
  client = new Client(app.origin);
  await client.signIn();
});

after(async () => {
  for (const id of made) await remove(id).catch(() => {});
  await app?.stop();
});

async function remove(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
  await client.call('DELETE', `/instances/${id}?delete_backups=true`);
}

const down = (id) => until('the server to be down', async () => !['starting', 'running', 'stopping'].includes((await client.call('GET', `/instances/${id}`)).body.state.status), 180);

/** A fresh server in offline mode, so the test player can join without an account, with its allowlist off. */
async function server(name, mcVersion) {
  const leftover = await client.findInstance(name);
  if (leftover) await remove(leftover.id);
  const created = await client.call('POST', '/instances', { name, mc_version: mcVersion, loader: 'vanilla', max_memory_mb: 1536, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const { id } = created.body.instance;
  made.push(id);
  const job = await client.waitForJob(created.body.job_id, 900);
  assert.equal(job.status, 'succeeded', job.error);
  await setProperty(id, 'online-mode', 'false');
  // 26.x starts with its allowlist on; the test turns it on itself when it wants it.
  await setProperty(id, 'white-list', 'false');
  return id;
}

async function setProperty(id, key, value) {
  const { body: file } = await client.call('GET', `/instances/${id}/files/text?path=server.properties`);
  const lines = file.content.split('\n').filter((line) => !line.startsWith(`${key}=`));
  const content = `${lines.join('\n').trimEnd()}\n${key}=${value}\n`;
  const saved = await client.call('PUT', `/instances/${id}/files/text`, { path: 'server.properties', revision: file.revision, content });
  assert.equal(saved.status, 200, JSON.stringify(saved.body));
}

async function start(id) {
  await client.call('POST', `/instances/${id}/start`);
  await client.waitForState(id, 'running', 420).catch(async (error) => {
    throw new Error(`${error.message}\n${(await client.console(id)).slice(-30).join('\n')}`);
  });
  return (await client.call('GET', `/instances/${id}`)).body.port;
}

async function stop(id) {
  await client.call('POST', `/instances/${id}/stop`);
  await down(id);
}

const players = async (id) => (await client.call('GET', `/instances/${id}/players`)).body;

/** Does it and expects it to work. */
async function act(id, action) {
  const { status, body } = await client.call('POST', `/instances/${id}/players/actions`, action);
  assert.equal(status, 200, `${JSON.stringify(action)}: ${JSON.stringify(body)}`);
  return body;
}

async function refused(id, action) {
  const { status, body } = await client.call('POST', `/instances/${id}/players/actions`, action);
  assert.notEqual(status, 200, JSON.stringify(action));
  return body.error.code;
}

/** The bot, in the world, and the app seeing it there. */
async function joined(id, port) {
  const bot = await joinServer({ port, name: BOT });
  await bot.joined;
  await until(`${BOT} to show as online`, async () => (await players(id)).online.some((player) => player.name === BOT), 30);
  return bot;
}

async function file(id, name) {
  return (await client.call('GET', `/instances/${id}/files/text?path=${name}`)).body.content;
}

test('26.3: a real player through the management protocol while it runs, and the files while it is stopped', { skip, timeout: 25 * 60_000 }, async (t) => {
  const id = await server('E2E players', process.env.E2E_MC ?? '26.3');
  let port = await start(id);
  try {
    const bot = await joined(id, port);
    const view = await players(id);
    assert.equal(view.channel, 'management');
    assert.equal(view.online[0].uuid, offlineUuid(BOT), 'the offline rule, as the server has it');
    assert.ok(view.online[0].since, 'its visit is being recorded');

    // It stays across keep-alives (sent every 15 seconds), as a real client does.
    await new Promise((resolve) => { setTimeout(resolve, 35_000); });
    assert.ok((await players(id)).online.some((player) => player.name === BOT), 'still on after 35 s');

    const opped = await act(id, { action: 'op', name: BOT, level: 3 });
    assert.equal(opped.via, 'management');
    assert.deepEqual(JSON.parse(await file(id, 'ops.json')).map(({ name, level }) => ({ name, level })), [{ name: BOT, level: 3 }], 'the server wrote it');
    await act(id, { action: 'deop', name: BOT });
    await act(id, { action: 'allow', name: 'Someone_Else' });
    await act(id, { action: 'disallow', name: 'Someone_Else' });
    await act(id, { action: 'message', name: BOT, text: 'Hello from the test' });

    const kicked = await act(id, { action: 'kick', name: BOT, reason: 'Kicked by the test' });
    assert.ok(!kicked.players.online.some((player) => player.name === BOT));
    const why = await bot.closed;
    t.diagnostic(`kicked: ${why}`);
    assert.match(why, /Kicked by the test/);

    // A ban that ends, which only the protocol (or the files) can make.
    const again = await joined(id, port);
    const expires = new Date(Date.now() + 60 * 60 * 1000).toISOString();
    const banned = await act(id, { action: 'ban', name: BOT, reason: 'Testing bans', expires });
    assert.equal(banned.players.bans[0].reason, 'Testing bans');
    assert.ok(Math.abs(new Date(banned.players.bans[0].expires) - new Date(expires)) < 2000, JSON.stringify(banned.players.bans[0]));
    t.diagnostic(`banned: ${await again.closed}`);
    const refusedJoin = await joinServer({ port, name: BOT });
    await assert.rejects(refusedJoin.joined, /banned/i, 'a banned player can not join');
    await act(id, { action: 'pardon', name: BOT });

    await act(id, { action: 'ban_ip', target: '10.9.8.7', reason: 'Addresses too' });
    assert.ok(JSON.parse(await file(id, 'banned-ips.json')).some((ban) => ban.ip === '10.9.8.7'));
    await act(id, { action: 'pardon_ip', ip: '10.9.8.7' });

    const on = await act(id, { action: 'allowlist', on: true });
    assert.equal(on.players.allowlist.on, true, 'the server saved the setting');
    const kept = await joinServer({ port, name: 'Not_Listed' });
    await assert.rejects(kept.joined, /white|allow/i, 'someone not on the list stays out');
    await act(id, { action: 'allowlist', on: false });

    const visits = (await client.call('GET', `/instances/${id}/players/${BOT}/sessions`)).body;
    t.diagnostic(`visits: ${JSON.stringify(visits.map(({ joined_at: joinedAt, left_at: leftAt }) => [joinedAt, leftAt]))}`);
    assert.ok(visits.filter((visit) => visit.left_at).length >= 2, JSON.stringify(visits));
  } finally {
    await stop(id);
  }

  // Stopped: the files, which the server reads when it starts again.
  const stopped = await players(id);
  assert.equal(stopped.channel, 'files');
  const bot = stopped.known.find((player) => player.name === BOT);
  t.diagnostic(`known: ${JSON.stringify(bot)}`);
  assert.ok(bot.played_seconds >= 35, 'the game’s own statistics count the time played');
  await act(id, { action: 'op', name: BOT, level: 2 });
  const ban = await act(id, { action: 'ban', name: 'Banned_Offline', reason: 'Written while stopped', expires: new Date(Date.now() + 86_400_000).toISOString() });
  assert.equal(ban.via, 'files');
  port = await start(id);
  try {
    const view = await players(id);
    assert.deepEqual(view.operators.map(({ name, level }) => ({ name, level })), [{ name: BOT, level: 2 }]);
    const banned = await joinServer({ port, name: 'Banned_Offline' });
    await assert.rejects(banned.joined, /banned/i, 'the server honours a ban written while it was stopped');
    await act(id, { action: 'pardon', name: 'Banned_Offline' });
    await act(id, { action: 'deop', name: BOT });
  } finally {
    await stop(id);
  }
});

test('1.20.1: a real player through the console', { skip, timeout: 25 * 60_000 }, async (t) => {
  const id = await server('E2E players old', '1.20.1');
  const port = await start(id);
  try {
    const bot = await joined(id, port);
    assert.equal((await players(id)).channel, 'console');
    await new Promise((resolve) => { setTimeout(resolve, 35_000); });
    assert.ok((await players(id)).online.some((player) => player.name === BOT), 'still on after 35 s');

    const opped = await act(id, { action: 'op', name: BOT });
    assert.equal(opped.via, 'console');
    assert.equal(opped.players.operators[0].name, BOT);
    assert.equal(await refused(id, { action: 'op', name: BOT, level: 2 }), 'needs_management_protocol');
    assert.equal(await refused(id, { action: 'ban', name: BOT, expires: new Date(Date.now() + 3_600_000).toISOString() }), 'needs_management_protocol');
    await act(id, { action: 'deop', name: BOT });
    await act(id, { action: 'allow', name: BOT });
    await act(id, { action: 'disallow', name: BOT });
    await act(id, { action: 'message', name: BOT, text: 'Hello from the console' });

    const kicked = await act(id, { action: 'kick', name: BOT, reason: 'Kicked through the console' });
    assert.ok(!kicked.players.online.some((player) => player.name === BOT));
    const why = await bot.closed;
    t.diagnostic(`kicked: ${why}`);
    assert.match(why, /Kicked through the console/);

    const again = await joined(id, port);
    await act(id, { action: 'ban', name: BOT, reason: 'Testing bans' });
    t.diagnostic(`banned: ${await again.closed}`);
    await act(id, { action: 'pardon', name: BOT });
    await act(id, { action: 'ban_ip', target: '10.9.8.7' });
    await act(id, { action: 'pardon_ip', ip: '10.9.8.7' });
    const on = await act(id, { action: 'allowlist', on: true });
    assert.equal(on.players.allowlist.on, true, 'the game saved `whitelist on` to its settings');
    await act(id, { action: 'allowlist', on: false });
  } finally {
    await stop(id);
  }
});

/** The pixels of an 8-bit RGBA PNG without interlacing (what the app serves), four bytes each. */
function rgbaPixels(png) {
  const width = png.readUInt32BE(16);
  const height = png.readUInt32BE(20);
  const data = [];
  for (let at = 8; at < png.length;) {
    const length = png.readUInt32BE(at);
    if (png.subarray(at + 4, at + 8).toString() === 'IDAT') data.push(png.subarray(at + 8, at + 8 + length));
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * 4;
  const out = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x += 1) {
      const value = raw[y * (stride + 1) + 1 + x];
      const left = x >= 4 ? out[y * stride + x - 4] : 0;
      const up = y > 0 ? out[(y - 1) * stride + x] : 0;
      const corner = x >= 4 && y > 0 ? out[(y - 1) * stride + x - 4] : 0;
      const paeth = () => {
        const p = left + up - corner;
        const [a, b, c] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - corner)];
        return a <= b && a <= c ? left : b <= c ? up : corner;
      };
      const predicted = [0, left, up, (left + up) >> 1, paeth()][filter];
      out[y * stride + x] = (value + predicted) & 0xff;
    }
  }
  return out;
}

test('Mojang: a name becomes its account for a stopped online-mode server, and an account has a head', { skip, timeout: 10 * 60_000 }, async (t) => {
  const id = made[0] ?? await server('E2E players', '26.3');
  await setProperty(id, 'online-mode', 'true');
  const allowed = await act(id, { action: 'allow', name: 'notch' });
  assert.deepEqual(allowed.players.allowlist.players.find((player) => player.uuid === NOTCH), { uuid: NOTCH, name: 'Notch' });
  assert.equal(await refused(id, { action: 'allow', name: 'zzNoSuchName9q' }), 'no_such_account');
  await act(id, { action: 'disallow', name: 'Notch' });

  const response = await fetch(`${app.origin}/api/v1/players/${NOTCH}/head.png`, { headers: { Cookie: client.cookie } });
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('content-type'), 'image/png');
  const png = Buffer.from(await response.arrayBuffer());
  assert.equal(png.subarray(1, 4).toString(), 'PNG');
  assert.equal(png.readUInt32BE(16), 64, 'width');
  assert.equal(png.readUInt32BE(20), 64, 'height');
  // A face, not one colour: Notch's old skin paints its hat area solid black, which the game leaves off.
  const pixels = rgbaPixels(png);
  const colours = new Set();
  for (let at = 0; at < pixels.length; at += 4) colours.add(pixels.readUInt32BE(at));
  t.diagnostic(`${colours.size} colours`);
  assert.ok(colours.size > 4, `a face has more than ${colours.size} colours`);
  assert.ok(![...colours].every((colour) => colour === 0x000000ff), 'not black');
  const offline = await fetch(`${app.origin}/api/v1/players/${offlineUuid(BOT)}/head.png`, { headers: { Cookie: client.cookie } });
  assert.equal(offline.status, 404, 'offline players have no skin of their own');
});
