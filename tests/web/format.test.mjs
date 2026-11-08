// Tests for web/js/core/format.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { relativeTime, untilTime, memoryLabel, softwareLabel, sizeLabel } = await import('../../web/js/core/format.js');

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

test('sizes use the largest sensible unit', () => {
  assert.equal(sizeLabel(0), '0 B');
  assert.equal(sizeLabel(1023), '1023 B');
  assert.equal(sizeLabel(1536), '1.5 KB');
  assert.equal(sizeLabel(120 * 1024 * 1024), '120 MB');
  assert.equal(sizeLabel(1.5 * 1024 ** 3), '1.5 GB');
  assert.equal(sizeLabel(3 * 1024 ** 5), '3072 TB', 'no unit beyond TB');
});

test('times ahead read naturally', () => {
  const now = Date.parse('2026-11-08T12:00:00Z');
  const ahead = (ms) => new Date(now + ms).toISOString();
  assert.equal(untilTime(ahead(10_000), now), 'in under a minute');
  assert.equal(untilTime(ahead(-5_000), now), 'in under a minute', 'due now');
  assert.equal(untilTime(ahead(5 * 60_000), now), 'in 5 min');
  assert.equal(untilTime(ahead(3 * 3_600_000), now), 'in 3 h');
  assert.match(untilTime(ahead(5 * 86_400_000), now), /^on .*2026/);
  assert.equal(untilTime('nope', now), '');
});
