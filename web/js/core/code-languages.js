/**
 * What the code editor highlights, as plain data (unit-tested in tests/web/code-languages.test.mjs):
 * which language a file is by its name, and simple highlighters for the formats CodeMirror has none
 * for. Those are CodeMirror "stream" parsers: `token(stream, state)` reads one token from a line and
 * says what it is (`keyword`, `string`, …); code-editor.js turns them into languages.
 *
 *   languageFor('config/paper-global.yml')   // 'yaml'
 *   languageFor('world/datapacks/x/data/f/function/a.mcfunction')   // 'mcfunction'
 */

const BY_EXTENSION = {
  json: 'json',
  json5: 'json',
  jsonc: 'json',
  mcmeta: 'json',
  yml: 'yaml',
  yaml: 'yaml',
  toml: 'toml',
  properties: 'properties',
  cfg: 'properties',
  ini: 'properties',
  conf: 'properties',
  mcfunction: 'mcfunction',
  snbt: 'snbt',
  log: 'log',
};

/** The language of a file by its name, or null for plain text. */
export function languageFor(path) {
  const name = (path ?? '').split('/').pop().toLowerCase();
  if (/\.log(\.\d+)?$/.test(name)) return 'log';
  const extension = name.includes('.') ? name.split('.').pop() : '';
  return BY_EXTENSION[extension] ?? null;
}

/** The languages whose files the editor checks before saving (see editor checks, D-058). */
export function checkedFormat(path) {
  const name = (path ?? '').split('/').pop().toLowerCase();
  if (/\.(json|mcmeta)$/.test(name)) return 'json';
  if (/\.(ya?ml)$/.test(name)) return 'yaml';
  if (/\.toml$/.test(name)) return 'toml';
  return null;
}

/** Function files: commands, one a line; `#` comments; selectors, positions, ids, NBT. */
export const mcfunction = {
  name: 'mcfunction',
  startState: () => ({ first: true }),
  token(stream, state) {
    if (stream.sol()) state.first = true;
    if (stream.eatSpace()) return null;
    if (state.first && stream.peek() === '#') {
      stream.skipToEnd();
      return 'comment';
    }
    if (state.first && stream.match(/^\$?[a-z_:.-]+/)) {
      state.first = false;
      return 'keyword';
    }
    state.first = false;
    return valueToken(stream);
  },
};

/** SNBT: `{key: "text", count: 3b, list: [1, 2]}`. */
export const snbt = {
  name: 'snbt',
  startState: () => ({}),
  token(stream) {
    if (stream.eatSpace()) return null;
    if (stream.match(/^[A-Za-z0-9_+.-]+(?=\s*:)/)) return 'propertyName';
    return valueToken(stream);
  },
};

/** Tokens shared by commands and SNBT. */
function valueToken(stream) {
  const next = stream.peek();
  if (next === '"' || next === "'") {
    stream.next();
    let escaped = false;
    for (let c = stream.next(); c !== undefined; c = stream.next()) {
      if (c === next && !escaped) break;
      escaped = !escaped && c === '\\';
    }
    return 'string';
  }
  if (stream.match(/^@[aenprs]\b/)) return 'variableName';
  if (stream.match(/^[~^]-?\d*\.?\d*/)) return 'number';
  if (stream.match(/^-?\d+(\.\d+)?[bBsSlLfFdD]?\b/)) return 'number';
  if (stream.match(/^(true|false)\b/)) return 'atom';
  if (stream.match(/^#?[a-z0-9_.-]+:[a-z0-9_./-]+/)) return 'string';
  if (stream.match(/^[[\]{}(),]/)) return 'bracket';
  if (stream.match(/^[=:;|]/)) return 'operator';
  stream.next();
  return null;
}

/** Server logs: `[12:00:00] [Server thread/WARN]: text`; stack traces; levels. */
export const log = {
  name: 'log',
  startState: () => ({ level: null }),
  token(stream, state) {
    if (stream.sol()) {
      state.level = null;
      if (stream.match(/^\s+at\s.*/) || stream.match(/^\s*\.\.\. \d+ more.*/)) return 'comment';
      if (stream.match(/^(Caused by|Exception in thread)\b.*/)) return 'invalid';
    }
    if (stream.match(/^\[[\d:. -]+\]/)) return 'meta';
    if (stream.eatSpace()) return null;
    const head = stream.match(/^\[([^\]/]+)\/(TRACE|DEBUG|INFO|WARN|ERROR|FATAL)\]/);
    if (head) {
      state.level = head[2];
      return levelToken(state.level);
    }
    stream.skipToEnd();
    return state.level === 'ERROR' || state.level === 'FATAL' ? 'invalid' : null;
  },
};

function levelToken(level) {
  if (level === 'WARN') return 'keyword';
  if (level === 'ERROR' || level === 'FATAL') return 'invalid';
  return 'meta';
}
