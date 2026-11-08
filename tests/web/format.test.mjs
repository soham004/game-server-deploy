// Tests for web/js/core/format.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { relativeTime, memoryLabel, softwareLabel } = await import('../../web/js/core/format.js');

test('relative times read naturally', () => {
  const now = Date.parse('2026-11-07T12:00:00Z');
  const ago = (ms) => new Date(now - ms).toISOString();
  assert.equal(relativeTime(ago(5_000), now), 'just now');
  assert.equal(relativeTime(ago(5 * 60_000), now), '5 min ago');
  assert.equal(relativeTime(ago(3 * 3_600_000), now), '3 h ago');
  assert.equal(relativeTime(ago(30 * 3_600_000), now), 'yesterday');
  assert.equal(relativeTime(ago(4 * 86_400_000), now), '4 days ago');
  assert.match(relativeTime(ago(30 * 86_400_000), now), /2026/);
  assert.equal(relativeTime(new Date(now + 60_000).toISOString(), now), 'just now', 'clock skew is not "in the future"');
  assert.equal(relativeTime('not a date', now), '');
});

test('memory and software labels', () => {
  assert.equal(memoryLabel(512), '512 MB');
  assert.equal(memoryLabel(2048), '2 GB');
  assert.equal(memoryLabel(1536), '1.5 GB');
  assert.equal(softwareLabel({ software: 'NeoForge 26.3' }), 'NeoForge 26.3');
  assert.equal(softwareLabel(null), '');
});
