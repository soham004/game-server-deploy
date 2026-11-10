// Tests for web/js/core/command-input.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { applySuggestion, describeStatus, isPreparing, moveSelection } = await import('../../web/js/core/command-input.js');

test('a suggestion replaces what it was for, then a space', () => {
  assert.deepEqual(applySuggestion('give @p dia', { text: 'diamond', from: 8, to: 11 }), { value: 'give @p diamond ', cursor: 16 });
  // The word went on past the cursor: all of it is replaced; the space after it is kept.
  assert.deepEqual(applySuggestion('gi @p', { text: 'give', from: 0, to: 2 }), { value: 'give @p', cursor: 5 });
  assert.deepEqual(applySuggestion('', { text: 'say', from: 0, to: 0 }), { value: 'say ', cursor: 4 });
  // Offsets are UTF-16, as JS counts.
  assert.deepEqual(applySuggestion('say 😀 @', { text: '@a', from: 7, to: 8 }), { value: 'say 😀 @a ', cursor: 10 });
});

test('the selection wraps around the list', () => {
  assert.equal(moveSelection(-1, 3, 1), 0);
  assert.equal(moveSelection(-1, 3, -1), 2);
  assert.equal(moveSelection(2, 3, 1), 0);
  assert.equal(moveSelection(0, 3, -1), 2);
  assert.equal(moveSelection(0, 0, 1), -1);
});

const ready = { vanilla: { state: 'ready', mc_version: '26.3', commands: 94 }, learned: { state: 'current' }, learned_commands: 2 };

test('nothing is said once commands are ready', () => {
  assert.equal(describeStatus(ready), null);
  assert.equal(describeStatus({ ...ready, learned: { state: 'stale' } }), null);
  assert.equal(describeStatus(null), null);
  assert.equal(isPreparing(ready), false);
});

test('preparing, learning and failures are explained', () => {
  const preparing = { ...ready, vanilla: { state: 'preparing', job_id: 'x' } };
  assert.match(describeStatus(preparing).text, /getting ready/);
  assert.equal(isPreparing(preparing), true);

  const learning = { ...ready, learned: { state: 'learning' } };
  assert.match(describeStatus(learning).text, /Asking the server/);
  assert.equal(isPreparing(learning), true);

  const failed = { ...ready, vanilla: { state: 'unavailable', reason: 'the data generator failed (exit code 1)' } };
  const message = describeStatus(failed);
  assert.match(message.text, /exit code 1.*help lists/);
  assert.equal(message.retry, true);
  assert.equal(describeStatus({ ...ready, learned: { state: 'failed', reason: 'no answer' } }).retry, true);
});
