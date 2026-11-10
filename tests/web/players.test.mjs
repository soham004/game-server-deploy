// Tests for web/js/core/players.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { banExpiry, channelNote, isPlayerName, playedLabel, playerMatches, secondsSince, standing } from '../../web/js/core/players.js';

test('ban lengths end when they say, and forever has no end', () => {
  const now = Date.UTC(2026, 10, 9, 12, 0, 0);
  assert.equal(banExpiry('forever', now), null);
  assert.equal(banExpiry('hour', now), '2026-11-09T13:00:00.000Z');
  assert.equal(banExpiry('week', now), '2026-11-16T12:00:00.000Z');
  assert.equal(banExpiry('nonsense', now), null);
});

test('play time reads naturally', () => {
  assert.equal(playedLabel(45), '45 s');
  assert.equal(playedLabel(12 * 60 + 5), '12 min');
  assert.equal(playedLabel(3 * 3600), '3 h');
  assert.equal(playedLabel(3 * 3600 + 5 * 60), '3 h 5 min');
  assert.equal(playedLabel(26 * 3600), '1 day 2 h');
  assert.equal(playedLabel(72 * 3600), '3 days');
  assert.equal(playedLabel(null), '');
  assert.equal(secondsSince('2026-11-09T12:00:00Z', Date.UTC(2026, 10, 9, 12, 1, 30)), 90);
  assert.equal(secondsSince('2030-01-01T00:00:00Z', Date.UTC(2026, 10, 9)), 0);
});

test('every channel says how changes are made', () => {
  for (const channel of ['management', 'console', 'files', null]) assert.ok(channelNote(channel).length > 20, channel);
  assert.match(channelNote('console'), /1\.21\.9/);
});

test('search by name or UUID, and names the server accepts', () => {
  const steve = { name: 'Steve', uuid: '8667ba71-b85a-4004-af54-457a9734eed7' };
  assert.ok(playerMatches(steve, 'ste'));
  assert.ok(playerMatches(steve, '8667BA'));
  assert.ok(playerMatches(steve, '  '));
  assert.ok(!playerMatches({ name: 'Alex', uuid: null }, 'ste'));
  assert.ok(isPlayerName('E2E_Bot'));
  for (const bad of ['', '@a', 'two words', 'abcdefghijklmnopq']) assert.ok(!isPlayerName(bad), bad);
});

test('a player’s standing comes from the lists, any case', () => {
  const view = {
    online: [{ name: 'Steve', since: '2026-11-09T12:00:00Z' }],
    operators: [{ name: 'steve', level: 3 }],
    allowlist: { players: [{ name: 'STEVE' }] },
    bans: [{ name: 'Griefer', reason: 'x' }],
  };
  const steve = standing(view, 'Steve');
  assert.equal(steve.operator.level, 3);
  assert.ok(steve.allowed && steve.online && !steve.ban);
  assert.equal(standing(view, 'griefer').ban.reason, 'x');
});
