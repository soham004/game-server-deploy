/**
 * The parts of console autocomplete that are plain logic (unit-tested in
 * tests/web/command-input.test.mjs): putting a suggestion into the text, moving through the list,
 * and saying where the server's commands stand.
 *
 *   const { value, cursor } = applySuggestion('give @p dia', { text: 'diamond', from: 8, to: 11 });
 *   // value 'give @p diamond ', cursor 16
 */

/**
 * Replaces `from..to` (UTF-16 offsets, as the server gives them and as JS strings count) with the
 * suggestion, then a space so the next part can be typed, unless one is there already.
 */
export function applySuggestion(value, { text, from, to }) {
  const before = value.slice(0, from);
  const after = value.slice(to);
  const space = after.startsWith(' ') ? '' : ' ';
  // The cursor goes past the space either way, ready for the next part.
  return { value: before + text + space + after, cursor: before.length + text.length + 1 };
}

/** The next selected index in a list of `count`, wrapping; -1 (nothing) moves to an end. */
export function moveSelection(current, count, delta) {
  if (count === 0) return -1;
  if (current < 0) return delta > 0 ? 0 : count - 1;
  return (current + delta + count) % count;
}

/**
 * What to tell the operator about autocomplete (`GET …/commands`), or null when it's ready and
 * there's nothing to say. `retry` says whether a "Try again" makes sense.
 */
export function describeStatus(status) {
  if (!status) return null;
  const { vanilla, learned } = status;
  if (vanilla.state === 'preparing') {
    return { text: 'Autocomplete is getting ready: reading this version’s commands (a minute or so, once).', retry: false };
  }
  if (vanilla.state === 'unavailable') {
    const fallback = status.learned_commands > 0 ? 'It knows only the commands the server’s help lists. ' : '';
    return { text: `Autocomplete couldn’t read this version’s commands: ${vanilla.reason}. ${fallback}`.trim(), retry: true };
  }
  if (learned.state === 'learning') {
    return { text: 'Asking the server for its mods’ and plugins’ commands…', retry: false };
  }
  if (learned.state === 'failed') {
    return { text: `Couldn’t learn the mods’ and plugins’ commands: ${learned.reason}.`, retry: true };
  }
  return null;
}

/** True while something is still being prepared, so the status is worth asking for again. */
export function isPreparing(status) {
  return status?.vanilla.state === 'preparing' || status?.learned.state === 'learning';
}
