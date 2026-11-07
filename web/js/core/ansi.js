/**
 * Renders terminal text with ANSI colour codes as DOM nodes.
 *
 *   element.append(...ansiToNodes('\x1b[92mgreen\x1b[0m plain'));
 *
 * Supports SGR ("select graphic rendition") codes: reset, bold, dim, italic, underline,
 * strikethrough, and foreground/background colours in the 16-colour (theme tokens, see
 * `.ansi-fg-N` in console.css), 256-colour and 24-bit forms. Every other escape or control
 * sequence (cursor moves, window titles, stray control characters) is removed. Text is inserted
 * as text nodes, never parsed as HTML.
 */

// CSI sequences (ESC [ params final; the final byte may be cut off at the end of a line),
// OSC sequences (ESC ] … BEL or ESC \), other two-byte escapes.
const ESCAPE = /\x1b(?:\[([0-9;:?]*)([@-~])?|\][^\x07\x1b]*(?:\x07|\x1b\\)?|[@-Z\\-_])/g;
// Control characters other than tab (newlines never reach a console line).
const CONTROL = /[\x00-\x08\x0a-\x1f\x7f]/g;

/**
 * @param {string} text
 * @returns {(Node)[]} text nodes and styled spans, in order
 */
export function ansiToNodes(text) {
  const nodes = [];
  let style = plainStyle();
  let pending = ''; // text in the current style, flushed when the style changes
  let index = 0;
  for (const match of text.matchAll(ESCAPE)) {
    pending += text.slice(index, match.index);
    index = match.index + match[0].length;
    if (match[2] === 'm' && !match[1].startsWith('?')) {
      pushText(nodes, pending, style);
      pending = '';
      style = applySgr(style, match[1]);
    }
  }
  pushText(nodes, pending + text.slice(index), style);
  return nodes;
}

function plainStyle() {
  return { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, strike: false };
}

/** Applies one SGR parameter list (e.g. "1;38;5;208") to a copy of `style`. */
function applySgr(previous, params) {
  const style = { ...previous };
  // Colon sub-parameters (38:2:r:g:b) are treated like semicolons; empty means 0.
  const codes = params === '' ? [0] : params.split(/[;:]/).map((p) => (p === '' ? 0 : Number(p)));
  for (let i = 0; i < codes.length; i += 1) {
    const code = codes[i];
    if (code === 0) Object.assign(style, plainStyle());
    else if (code === 1) style.bold = true;
    else if (code === 2) style.dim = true;
    else if (code === 3) style.italic = true;
    else if (code === 4) style.underline = true;
    else if (code === 9) style.strike = true;
    else if (code === 22) { style.bold = false; style.dim = false; }
    else if (code === 23) style.italic = false;
    else if (code === 24) style.underline = false;
    else if (code === 29) style.strike = false;
    else if (code >= 30 && code <= 37) style.fg = { index: code - 30 };
    else if (code >= 90 && code <= 97) style.fg = { index: code - 90 + 8 };
    else if (code === 39) style.fg = null;
    else if (code >= 40 && code <= 47) style.bg = { index: code - 40 };
    else if (code >= 100 && code <= 107) style.bg = { index: code - 100 + 8 };
    else if (code === 49) style.bg = null;
    else if (code === 38 || code === 48) {
      const [color, used] = extendedColor(codes, i + 1);
      // Malformed: the remaining numbers belong to it, so don't read them as other codes.
      if (color === null) break;
      if (code === 38) style.fg = color;
      else style.bg = color;
      i += used;
    }
  }
  return style;
}

/** Parses `5;n` or `2;r;g;b` starting at `start`; returns [colour or null, codes consumed]. */
function extendedColor(codes, start) {
  if (codes[start] === 5 && codes[start + 1] !== undefined) {
    return [color256(codes[start + 1]), 2];
  }
  if (codes[start] === 2 && codes[start + 3] !== undefined) {
    const [r, g, b] = codes.slice(start + 1, start + 4).map(clampByte);
    return [{ rgb: `rgb(${r}, ${g}, ${b})` }, 4];
  }
  return [null, 0];
}

function color256(n) {
  if (n < 16) return { index: n };
  if (n < 232) {
    const levels = [0, 95, 135, 175, 215, 255];
    const cube = n - 16;
    const r = levels[Math.floor(cube / 36)];
    const g = levels[Math.floor(cube / 6) % 6];
    const b = levels[cube % 6];
    return { rgb: `rgb(${r}, ${g}, ${b})` };
  }
  const gray = clampByte(8 + (n - 232) * 10);
  return { rgb: `rgb(${gray}, ${gray}, ${gray})` };
}

function clampByte(value) {
  return Math.max(0, Math.min(255, Number.isFinite(value) ? value : 0));
}

function pushText(nodes, raw, style) {
  const text = raw.replace(CONTROL, '');
  if (text === '') return;
  const classes = [];
  if (style.fg?.index !== undefined) classes.push(`ansi-fg-${style.fg.index}`);
  if (style.bg?.index !== undefined) classes.push(`ansi-bg-${style.bg.index}`);
  if (style.bold) classes.push('ansi-bold');
  if (style.dim) classes.push('ansi-dim');
  if (style.italic) classes.push('ansi-italic');
  if (style.underline) classes.push('ansi-underline');
  if (style.strike) classes.push('ansi-strike');
  const hasRgb = Boolean(style.fg?.rgb || style.bg?.rgb);
  if (classes.length === 0 && !hasRgb) {
    nodes.push(document.createTextNode(text));
    return;
  }
  const span = document.createElement('span');
  span.className = classes.join(' ');
  // CSSOM assignments, not style attributes, so the strict CSP still allows them.
  if (style.fg?.rgb) span.style.color = style.fg.rgb;
  if (style.bg?.rgb) span.style.backgroundColor = style.bg.rgb;
  span.append(text);
  nodes.push(span);
}
