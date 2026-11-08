// Tests for web/js/core/schedule-plan.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const {
  parseWarnings, formatWarnings, dailyCron, dailyTime, splitDuration, templates, blankPlan, actionsSummary,
} = await import('../../web/js/core/schedule-plan.js');

test('warning lists read and write in people\'s units', () => {
  assert.deepEqual(parseWarnings('10m, 5m, 1m, 10s'), [600, 300, 60, 10]);
  assert.deepEqual(parseWarnings('600 300  60'), [600, 300, 60], 'plain numbers are seconds');
  assert.deepEqual(parseWarnings('1h,30min'), [3600, 1800]);
  assert.deepEqual(parseWarnings('  '), []);
  assert.throws(() => parseWarnings('soon'), /not a time/);
  assert.equal(formatWarnings([3600, 600, 90, 10]), '1h, 10m, 90s, 10s');
  assert.deepEqual(parseWarnings(formatWarnings([600, 300, 60, 10])), [600, 300, 60, 10], 'round trip');
});

test('"every day at" maps to a cron expression and back', () => {
  assert.equal(dailyCron('04:30'), '30 4 * * *');
  assert.equal(dailyCron('0:05'), '5 0 * * *');
  assert.throws(() => dailyCron(''), /time of day/);
  assert.equal(dailyTime('30 4 * * *'), '04:30');
  assert.equal(dailyTime('0 */6 * * *'), null, 'not a daily pattern');
  assert.equal(dailyTime(dailyCron('23:59')), '23:59');
});

test('intervals are shown in the largest even unit', () => {
  assert.deepEqual(splitDuration(21600), { count: 6, unit: 3600 });
  assert.deepEqual(splitDuration(86400 * 2), { count: 2, unit: 86400 });
  assert.deepEqual(splitDuration(1800), { count: 30, unit: 60 });
});

test('templates and the blank plan are complete plans in the browser\'s time zone', () => {
  const all = templates('Asia/Kolkata');
  assert.equal(all.length, 4);
  for (const { plan } of all) {
    assert.ok(plan.name && plan.trigger && plan.actions.length > 0, plan.name);
    assert.deepEqual(Object.keys(plan).sort(), ['actions', 'catch_up', 'conditions', 'enabled', 'name', 'on_error', 'trigger']);
  }
  assert.equal(all[0].plan.trigger.time_zone, 'Asia/Kolkata');
  assert.equal(blankPlan('UTC').trigger.time_zone, 'UTC');
});

test('actions are summarised in order', () => {
  assert.equal(
    actionsSummary([{ kind: 'restart', warn_secs: [600, 60] }, { kind: 'backup', scope: 'world' }]),
    'Restart (warnings 10m, 1m) → Backup (worlds)',
  );
  assert.equal(actionsSummary([{ kind: 'command', command: 'save-all' }]), 'Command: save-all');
});
