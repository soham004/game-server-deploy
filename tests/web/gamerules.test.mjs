// Tests for web/js/core/gamerules.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { GROUP_LABELS, isChanged, pendingChanges, ruleMatches, valuesNote } from '../../web/js/core/gamerules.js';

const keep = { name: 'keep_inventory', type: 'bool', value: false, default: false, label: 'Keep inventory', description: 'Players keep their items' };
const tick = { name: 'random_tick_speed', type: 'int', value: 3, default: 3, label: 'Random tick speed', description: 'How fast crops grow' };
const modded = { name: 'somemod:faster', type: 'int', value: 2, default: null, label: null, description: null };

test('only real changes are sent, and numbers must be whole 32-bit numbers', () => {
  const drafts = new Map([['keep_inventory', true], ['random_tick_speed', '3'], ['somemod:faster', ' 5 ']]);
  assert.deepEqual(pendingChanges([keep, tick, modded], drafts), { changes: { keep_inventory: true, 'somemod:faster': 5 }, invalid: [] });
  for (const bad of ['', '1.5', 'abc', '3000000000']) {
    assert.deepEqual(pendingChanges([tick], new Map([['random_tick_speed', bad]])).invalid, ['random_tick_speed'], bad);
  }
  assert.deepEqual(pendingChanges([tick], new Map([['random_tick_speed', '-1']])).changes, { random_tick_speed: -1 });
});

test('changed from the default, where there is one', () => {
  assert.ok(!isChanged(keep));
  assert.ok(isChanged(keep, true));
  assert.ok(!isChanged(modded, 99), 'no default, nothing to compare with');
});

test('search covers labels, names and descriptions', () => {
  assert.ok(ruleMatches(keep, 'INVENTORY'));
  assert.ok(ruleMatches(tick, 'crops'));
  assert.ok(ruleMatches(modded, 'somemod'));
  assert.ok(!ruleMatches(keep, 'crops'));
  assert.ok(ruleMatches(keep, '  '));
});

test('every state says where values come from', () => {
  assert.match(valuesNote({ values: null, channel: 'files' }), /no world/);
  assert.match(valuesNote({ values: 'server', channel: 'management' }), /Live/);
  assert.match(valuesNote({ values: 'last_save', channel: 'console' }), /last saved/);
  assert.match(valuesNote({ values: 'files', channel: 'files' }), /stopped/);
  assert.match(valuesNote({ values: 'files', channel: null }), /Wait/);
  assert.equal(Object.keys(GROUP_LABELS).length, 8);
});
