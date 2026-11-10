// A pretend player for the end-to-end tests: just enough of the Minecraft protocol to join a server in
// offline mode (`online-mode=false`), stay as long as a test needs (it answers keep-alives), and leave, so the
// app sees a real player come and go, and a real kick or ban drop it. No dependencies.
//
//   const bot = await joinServer({ port: 25565, name: 'E2E_Bot' });
//   await bot.joined;          // in the world
//   … await bot.closed …       // the reason, when the server drops it (a kick, a ban)
//   bot.leave();
//
// It reads the protocol number from the server's status reply and knows the packet ids of the versions the
// tests use (PROTOCOLS). For another version, take them from the data generator's `reports/packets.json`:
//   java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --output <folder>

import { createHash } from 'node:crypto';
import { connect } from 'node:net';
import { deflateSync, inflateSync } from 'node:zlib';

/**
 * Packet ids by protocol number. `configuration` is null for versions before 1.20.2, which go from logging
 * in straight to playing. Login ids have stayed the same throughout.
 */
const PROTOCOLS = {
  // 1.20.1
  763: { configuration: null, play: { disconnect: 0x1a, keepAlive: 0x23, keepAliveOut: 0x12 } },
  // 26.3
  777: {
    configuration: { disconnect: 0x02, finish: 0x03, keepAlive: 0x04, ping: 0x05, knownPacks: 0x0f, finished: 0x03, keepAliveOut: 0x04, pong: 0x05, knownPacksOut: 0x07 },
    play: { disconnect: 0x20, keepAlive: 0x2d, keepAliveOut: 0x1c },
  },
};
const LOGIN = { disconnect: 0x00, encryption: 0x01, success: 0x02, compression: 0x03, pluginRequest: 0x04, start: 0x00, pluginAnswer: 0x02, acknowledged: 0x03 };

/** The UUID an offline-mode server gives a name: Java's `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`. */
export function offlineUuid(name) {
  const hash = createHash('md5').update(`OfflinePlayer:${name}`).digest();
  hash[6] = (hash[6] & 0x0f) | 0x30;
  hash[8] = (hash[8] & 0x3f) | 0x80;
  const hex = hash.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function varInt(value) {
  const bytes = [];
  let rest = value >>> 0;
  do {
    let byte = rest & 0x7f;
    rest >>>= 7;
    if (rest) byte |= 0x80;
    bytes.push(byte);
  } while (rest);
  return Buffer.from(bytes);
}

/** A VarInt at `offset`: `[value, bytes used]`, or null if the buffer ends first. */
function readVarInt(buffer, offset = 0) {
  let value = 0;
  for (let i = 0; i < 5; i += 1) {
    if (offset + i >= buffer.length) return null;
    const byte = buffer[offset + i];
    value |= (byte & 0x7f) << (7 * i);
    if (!(byte & 0x80)) return [value, i + 1];
  }
  throw new Error('a VarInt that is too long');
}

const string = (text) => Buffer.concat([varInt(Buffer.byteLength(text)), Buffer.from(text)]);
const port16 = (port) => Buffer.from([port >> 8, port & 255]);

/** Splits packets off the front of a stream; `onPacket(id, data)` for each whole one. */
function packetReader(compressionOf, onPacket) {
  let buffer = Buffer.alloc(0);
  return (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const length = readVarInt(buffer);
      if (!length || buffer.length < length[1] + length[0]) return;
      let packet = buffer.subarray(length[1], length[1] + length[0]);
      buffer = buffer.subarray(length[1] + length[0]);
      if (compressionOf() >= 0) {
        const [size, used] = readVarInt(packet);
        packet = size === 0 ? packet.subarray(used) : inflateSync(packet.subarray(used));
      }
      const [id, used] = readVarInt(packet);
      onPacket(id, packet.subarray(used));
    }
  };
}

/** The server's status reply's `version`: `{ name, protocol }`. */
export function serverStatus({ host = '127.0.0.1', port }) {
  return new Promise((resolve, reject) => {
    const socket = connect({ host, port });
    socket.setTimeout(10_000, () => socket.destroy(new Error('no status reply')));
    socket.on('error', reject);
    socket.on('connect', () => {
      const handshake = Buffer.concat([varInt(0), varInt(-1), string(host), port16(port), varInt(1)]);
      socket.write(Buffer.concat([varInt(handshake.length), handshake, varInt(1), varInt(0)]));
    });
    socket.on('data', packetReader(() => -1, (_id, data) => {
      const [length, used] = readVarInt(data);
      socket.destroy();
      resolve(JSON.parse(data.subarray(used, used + length).toString()).version);
    }));
  });
}

/** Readable words from a disconnect packet: JSON text while logging in (and in 1.20.1), NBT text later. */
function reasonIn(data) {
  return data.toString('utf8').replace(/[^\x20-\x7e]+/g, ' ').trim() || 'no reason given';
}

/**
 * Joins as `name`. Resolves once connected; `joined` resolves in the world and `closed` with why the
 * connection ended, in the server's own words when it gave any.
 */
export async function joinServer({ host = '127.0.0.1', port, name }) {
  const { protocol, name: version } = await serverStatus({ host, port });
  const ids = PROTOCOLS[protocol];
  if (!ids) throw new Error(`the test player doesn't know protocol ${protocol} (${version}): add its packet ids to PROTOCOLS`);
  const socket = connect({ host, port });
  await new Promise((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); });

  let compression = -1;
  let phase = 'login';
  let reason = 'the connection closed';
  let markJoined;
  let failJoin;
  const joined = new Promise((resolve, reject) => { markJoined = resolve; failJoin = reject; });
  joined.catch(() => {});
  const closed = new Promise((resolve) => {
    socket.on('close', () => { failJoin(new Error(`${name} could not join: ${reason}`)); resolve(reason); });
  });
  socket.on('error', (error) => { reason = error.message; });
  const drop = (why) => { reason = why; socket.destroy(); };

  const send = (id, ...parts) => {
    const body = Buffer.concat([varInt(id), ...parts]);
    let packet = body;
    if (compression >= 0) packet = body.length >= compression ? Buffer.concat([varInt(body.length), deflateSync(body)]) : Buffer.concat([varInt(0), body]);
    socket.write(Buffer.concat([varInt(packet.length), packet]));
  };

  const play = () => { phase = 'play'; markJoined(); };

  const onLogin = (id, data) => {
    if (id === LOGIN.compression) compression = readVarInt(data)[0];
    else if (id === LOGIN.encryption) drop('the server is in online mode');
    else if (id === LOGIN.disconnect) drop(reasonIn(data));
    else if (id === LOGIN.pluginRequest) send(LOGIN.pluginAnswer, varInt(readVarInt(data)[0]), Buffer.from([0]));
    else if (id === LOGIN.success && ids.configuration) {
      send(LOGIN.acknowledged);
      phase = 'configuration';
    } else if (id === LOGIN.success) play();
  };

  const onConfiguration = (id, data) => {
    const config = ids.configuration;
    // No packs known: the server then sends its registries in full, which are ignored.
    if (id === config.knownPacks) send(config.knownPacksOut, varInt(0));
    else if (id === config.keepAlive) send(config.keepAliveOut, data);
    else if (id === config.ping) send(config.pong, data);
    else if (id === config.disconnect) drop(reasonIn(data));
    else if (id === config.finish) {
      send(config.finished);
      play();
    }
  };

  const onPlay = (id, data) => {
    if (id === ids.play.keepAlive) send(ids.play.keepAliveOut, data);
    else if (id === ids.play.disconnect) drop(reasonIn(data));
  };

  socket.on('data', packetReader(() => compression, (id, data) => {
    if (phase === 'login') onLogin(id, data);
    else if (phase === 'configuration') onConfiguration(id, data);
    else onPlay(id, data);
  }));

  const handshake = Buffer.concat([varInt(0), varInt(protocol), string(host), port16(port), varInt(2)]);
  socket.write(Buffer.concat([varInt(handshake.length), handshake]));
  const uuid = offlineUuid(name);
  const uuidBytes = Buffer.from(uuid.replaceAll('-', ''), 'hex');
  // 1.20.1 says whether a UUID follows; later versions always send one.
  if (ids.configuration) send(LOGIN.start, string(name), uuidBytes);
  else send(LOGIN.start, string(name), Buffer.from([1]), uuidBytes);

  return { name, uuid, protocol, version, joined, closed, leave: () => socket.destroy() };
}
