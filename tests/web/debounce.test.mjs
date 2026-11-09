// Tests for web/js/core/debounce.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { mock, test } from 'node:test';

const { debounce } = await import('../../web/js/core/debounce.js');

test('runs once, after the calls stop, with the last arguments', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const fn = mock.fn();
  const later = debounce(fn, 200);
  later('l');
  t.mock.timers.tick(150);
  later('li');
  t.mock.timers.tick(150);
  later('lit');
  assert.equal(fn.mock.callCount(), 0, 'still typing');
  t.mock.timers.tick(200);
  assert.equal(fn.mock.callCount(), 1);
  assert.deepEqual(fn.mock.calls[0].arguments, ['lit']);
});

test('a cancelled call never runs', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const fn = mock.fn();
  const later = debounce(fn, 200);
  later();
  later.cancel();
  t.mock.timers.tick(1000);
  assert.equal(fn.mock.callCount(), 0);
  later();
  t.mock.timers.tick(200);
  assert.equal(fn.mock.callCount(), 1, 'usable again after cancelling');
});
