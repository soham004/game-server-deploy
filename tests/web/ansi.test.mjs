// Tests for web/js/core/ansi.js. Run: node --test tests/web/
// Uses a minimal DOM stand-in (text nodes and spans are all the module creates).

import assert from 'node:assert/strict';
import { test } from 'node:test';

class FakeText {
  constructor(text) {
    this.textContent = text;
  }
}

class FakeElement {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.className = '';
    this.style = {};
    this.textContent = '';
  }

  append(text) {
    this.textContent += text;
  }
}

globalThis.document = {
  createTextNode: (text) => new FakeText(text),
  createElement: (tag) => new FakeElement(tag),
};

const { ansiToNodes } = await import('../../web/js/core/ansi.js');

/** Compact view: plain strings for text nodes, [className, text, style] for spans. */
function render(text) {
  return ansiToNodes(text).map((node) =>
    node instanceof FakeText
      ? node.textContent
      : [node.className, node.textContent, Object.keys(node.style).length ? node.style : undefined].filter(
          (part) => part !== undefined,
        ),
  );
}

test('real Paper output: a bright green seed', () => {
  const line = '[14:32:55 INFO]: System chat: Seed: [\x1b[92m-7919143748959950447\x1b[0m]';
  assert.deepEqual(render(line), [
    '[14:32:55 INFO]: System chat: Seed: [',
    ['ansi-fg-10', '-7919143748959950447'],
    ']',
  ]);
});

test('plain text stays a single text node', () => {
  assert.deepEqual(render('There are 0 of a max of 20 players online:'), [
    'There are 0 of a max of 20 players online:',
  ]);
});

test('normal and bright colours, background, and resets', () => {
  assert.deepEqual(render('\x1b[31mred\x1b[39m plain \x1b[44;97mbox\x1b[49m tail\x1b[0m'), [
    ['ansi-fg-1', 'red'],
    ' plain ',
    ['ansi-fg-15 ansi-bg-4', 'box'],
    ['ansi-fg-15', ' tail'],
  ]);
});

test('text attributes combine and turn off independently', () => {
  assert.deepEqual(render('\x1b[1;3;4mall\x1b[22mno-bold\x1b[23;24m\x1b[9mstruck\x1b[m'), [
    ['ansi-bold ansi-italic ansi-underline', 'all'],
    ['ansi-italic ansi-underline', 'no-bold'],
    ['ansi-strike', 'struck'],
  ]);
});

test('24-bit colours (Adventure hex colours) use inline colour', () => {
  assert.deepEqual(render('\x1b[38;2;255;170;0mgold\x1b[0m'), [['', 'gold', { color: 'rgb(255, 170, 0)' }]]);
  assert.deepEqual(render('\x1b[38:2:1:2:3mcolon\x1b[0m'), [['', 'colon', { color: 'rgb(1, 2, 3)' }]]);
});

test('256-colour palette: standard, cube and grey ramp', () => {
  assert.deepEqual(render('\x1b[38;5;9ma'), [['ansi-fg-9', 'a']]);
  assert.deepEqual(render('\x1b[38;5;208mb'), [['', 'b', { color: 'rgb(255, 135, 0)' }]]);
  assert.deepEqual(render('\x1b[48;5;232mc'), [['', 'c', { backgroundColor: 'rgb(8, 8, 8)' }]]);
});

test('non-colour escapes and control characters are removed', () => {
  const line = '\x1b]0;window title\x07\x1b[2K\x1b[1Gdone\x1b[?25h\r\x00\x07!';
  assert.deepEqual(render(line), ['done!']);
});

test('a lone or truncated escape leaves no invisible characters', () => {
  assert.deepEqual(render('a\x1bb\x1b['), ['ab']);
});

test('malformed extended colours are ignored without breaking the rest', () => {
  assert.deepEqual(render('\x1b[38;5mx\x1b[38;2;1mz'), ['x', 'z']);
});
