/**
 * What the game rules tab works out without the page: group names, the note on where values come from,
 * searching, and which edits are real changes. Kept apart from the DOM so it can be tested on its own.
 */

export const GROUP_LABELS = {
  player: 'Players',
  mobs: 'Mobs',
  spawning: 'Spawning',
  drops: 'Drops',
  world: 'World',
  chat: 'Chat and messages',
  misc: 'Commands',
  other: 'Other (from mods or a newer version)',
};

/** One sentence on where the values come from and what a change does. */
export function valuesNote(view) {
  if (view.values === null) return 'This server has no world yet. It makes one, and its game rules, when it first starts.';
  if (view.channel === null) return 'Wait until the server has started or stopped to make changes.';
  switch (view.values) {
    case 'server':
      return 'Live from the running server. Changes apply at once.';
    case 'last_save':
      return 'As the running server last saved them (it saves every few minutes). Changes are typed into its console, then it saves.';
    default:
      return 'The server is stopped: changes go into its world and apply when it starts.';
  }
}

/** Whether a rule's label, name or description contains the search (any case). */
export function ruleMatches(rule, query) {
  const wanted = query.trim().toLowerCase();
  if (!wanted) return true;
  return [rule.label, rule.name, rule.description].some((text) => (text ?? '').toLowerCase().includes(wanted));
}

/** Whether a value differs from the rule's known default; false when there is none to compare with. */
export function isChanged(rule, value = rule.value) {
  return rule.default !== null && rule.default !== undefined && value !== rule.default;
}

/**
 * The edits that change something: `{ name: value }`. A number field's text becomes a whole number;
 * one that isn't is left out and reported in `invalid`.
 * @param {{ name: string, type: string, value: boolean|number }[]} rules
 * @param {Map<string, boolean|string>} drafts what the page holds, by rule name
 */
export function pendingChanges(rules, drafts) {
  const changes = {};
  const invalid = [];
  for (const rule of rules) {
    if (!drafts.has(rule.name)) continue;
    let value = drafts.get(rule.name);
    if (rule.type === 'int') {
      const text = String(value).trim();
      if (!/^-?\d+$/.test(text) || Math.abs(Number(text)) > 2147483647) {
        invalid.push(rule.name);
        continue;
      }
      value = Number(text);
    }
    if (value !== rule.value) changes[rule.name] = value;
  }
  return { changes, invalid };
}
