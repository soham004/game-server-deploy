// Tests for web/js/core/code-languages.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { checkedFormat, languageFor, log, mcfunction, snbt } = await import('../../web/js/core/code-languages.js');

test('a file’s language follows its name', () => {
  assert.equal(languageFor('config/paper-global.yml'), 'yaml');
  assert.equal(languageFor('pack.mcmeta'), 'json');
  assert.equal(languageFor('config/sodium.json5'), 'json');
  assert.equal(languageFor('config/forge-server.TOML'), 'toml');
  assert.equal(languageFor('server.properties'), 'properties');
  assert.equal(languageFor('config/mod.cfg'), 'properties');
  assert.equal(languageFor('world/datapacks/p/data/n/function/tick.mcfunction'), 'mcfunction');
  assert.equal(languageFor('logs/latest.log'), 'log');
  assert.equal(languageFor('logs/debug.log.1'), 'log');
  assert.equal(languageFor('eula.txt'), null);
  assert.equal(languageFor('Makefile'), null);
  assert.equal(languageFor(undefined), null);
});

test('only JSON, YAML and TOML are checked', () => {
  assert.equal(checkedFormat('a/b.json'), 'json');
  assert.equal(checkedFormat('pack.mcmeta'), 'json');
  assert.equal(checkedFormat('bukkit.yml'), 'yaml');
  assert.equal(checkedFormat('x.yaml'), 'yaml');
  assert.equal(checkedFormat('x.toml'), 'toml');
  // JSON5 allows comments and trailing commas: not checked as JSON.
  assert.equal(checkedFormat('x.json5'), null);
  assert.equal(checkedFormat('server.properties'), null);
});

/** The little of CodeMirror's StringStream the parsers use, for one line. */
class Stream {
  constructor(line) {
    this.string = line;
    this.pos = 0;
    this.start = 0;
  }
  sol() { return this.pos === 0; }
  eol() { return this.pos >= this.string.length; }
  peek() { return this.string.charAt(this.pos) || undefined; }
  next() { return this.pos < this.string.length ? this.string.charAt(this.pos++) : undefined; }
  eatSpace() {
    const start = this.pos;
    while (/\s/.test(this.string.charAt(this.pos))) this.pos += 1;
    return this.pos > start;
  }
  skipToEnd() { this.pos = this.string.length; }
  match(pattern) {
    const found = this.string.slice(this.pos).match(pattern);
    if (!found || found.index !== 0) return null;
    this.pos += found[0].length;
    return found;
  }
  current() { return this.string.slice(this.start, this.pos); }
}

/** [text, style] pairs for one line, skipping unstyled spaces. */
function tokens(language, line) {
  const stream = new Stream(line);
  const state = language.startState();
  const found = [];
  while (!stream.eol()) {
    stream.start = stream.pos;
    const style = language.token(stream, state);
    assert.ok(stream.pos > stream.start, `the parser moved on at ${stream.pos} in ${line}`);
    const text = stream.current();
    if (text.trim() || style) found.push([text, style]);
  }
  return found;
}

test('function files: the command, selectors, positions, ids, values', () => {
  assert.deepEqual(tokens(mcfunction, 'give @p minecraft:diamond 64'), [
    ['give', 'keyword'], ['@p', 'variableName'], ['minecraft:diamond', 'string'], ['64', 'number'],
  ]);
  assert.deepEqual(tokens(mcfunction, 'tp @s ~ ~1 ^-2'), [
    ['tp', 'keyword'], ['@s', 'variableName'], ['~', 'number'], ['~1', 'number'], ['^-2', 'number'],
  ]);
  assert.deepEqual(tokens(mcfunction, '# a comment'), [['# a comment', 'comment']]);
  assert.deepEqual(tokens(mcfunction, 'say "hi there"').map(([, style]) => style), ['keyword', 'string']);
});

test('SNBT: keys, numbers with their suffix, strings', () => {
  assert.deepEqual(tokens(snbt, '{Count: 3b, id: "minecraft:stone", Damage: 1.5f, ok: true}'), [
    ['{', 'bracket'], ['Count', 'propertyName'], [':', 'operator'], ['3b', 'number'], [',', 'bracket'],
    ['id', 'propertyName'], [':', 'operator'], ['"minecraft:stone"', 'string'], [',', 'bracket'],
    ['Damage', 'propertyName'], [':', 'operator'], ['1.5f', 'number'], [',', 'bracket'],
    ['ok', 'propertyName'], [':', 'operator'], ['true', 'atom'], ['}', 'bracket'],
  ]);
});

test('logs: the time, the thread and level, warnings and errors', () => {
  assert.deepEqual(tokens(log, '[12:00:01] [Server thread/INFO]: Done (1.2s)!'), [
    ['[12:00:01]', 'meta'], ['[Server thread/INFO]', 'meta'], [': Done (1.2s)!', null],
  ]);
  assert.deepEqual(tokens(log, '[12:00:01] [Server thread/WARN]: Can’t keep up!').map(([, style]) => style), ['meta', 'keyword', null]);
  assert.deepEqual(tokens(log, '[12:00:01] [main/ERROR]: boom').map(([, style]) => style), ['meta', 'invalid', 'invalid']);
  assert.deepEqual(tokens(log, '\tat net.minecraft.Main.main(Main.java:1)'), [['\tat net.minecraft.Main.main(Main.java:1)', 'comment']]);
  assert.deepEqual(tokens(log, 'Caused by: java.io.IOException'), [['Caused by: java.io.IOException', 'invalid']]);
});
