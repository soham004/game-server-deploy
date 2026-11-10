/**
 * A server's game rules: grouped, searchable, each with what it does and whether it differs from the
 * default. Edits are held on the page and saved together; the server's own values come back afterwards.
 *
 *   const rules = createGamerules({ instanceId, canWrite });
 *   panel.append(rules.element);
 *   rules.load();        // on opening the tab
 *   rules.refresh();     // when the server starts or stops (where values come from changes)
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { GROUP_LABELS, isChanged, pendingChanges, ruleMatches, valuesNote } from '../core/gamerules.js';
import { toast } from '../core/toast.js';

const VIA = {
  management: 'through the management protocol',
  console: 'through the console',
  files: 'in the world’s files',
};

export function createGamerules({ instanceId, canWrite }) {
  const note = h('p', { class: 'c-muted c-gamerules__note', role: 'status' });
  const search = h('input', { class: 'c-input', type: 'search', name: 'gamerule_search', placeholder: 'Find a rule', 'aria-label': 'Find a rule' });
  const changedOnly = h('input', { type: 'checkbox', name: 'gamerule_changed_only' });
  const count = h('span', { class: 'c-muted' });
  const save = h('button', { class: 'c-button c-button--primary', type: 'button', disabled: true, onClick: () => submit() }, 'Save');
  const discard = h('button', { class: 'c-button c-button--ghost', type: 'button', disabled: true, onClick: () => { drafts.clear(); render(); } }, 'Discard');
  const groups = h('div', { class: 'c-gamerules__groups' });
  const element = h('section', { class: 'c-gamerules' },
    note,
    h('div', { class: 'c-gamerules__tools' },
      search,
      h('label', { class: 'c-check' }, changedOnly, h('span', null, 'Only those changed from the default')),
      canWrite ? h('div', { class: 'c-gamerules__save' }, count, discard, save) : null),
    groups);

  let view = null;
  let loaded = false;
  /** Edits not saved yet, by rule name: true/false, or a number field's text. */
  const drafts = new Map();
  let latestRead = 0;

  async function load() {
    loaded = true;
    await refresh();
  }

  async function refresh() {
    if (!loaded) return;
    const thisRead = ++latestRead;
    try {
      const fresh = await api.get(`/instances/${instanceId}/gamerules`);
      if (thisRead !== latestRead) return;
      view = fresh;
      render();
    } catch (error) {
      if (thisRead === latestRead) note.textContent = error.message;
    }
  }

  async function submit() {
    const { changes, invalid } = pendingChanges(view.rules, drafts);
    if (invalid.length) {
      toast(`Not a whole number: ${invalid.join(', ')}`, { kind: 'error' });
      return;
    }
    if (Object.keys(changes).length === 0) return;
    save.disabled = true;
    try {
      const updated = await api.patch(`/instances/${instanceId}/gamerules`, { changes });
      ++latestRead;
      view = updated.gamerules;
      drafts.clear();
      toast(`Saved ${VIA[updated.via] ?? ''}.`.replace(' .', '.'), { kind: 'success' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    render();
  }

  function render() {
    note.textContent = valuesNote(view);
    const editable = canWrite && view.channel !== null && view.values !== null;
    const { changes } = pendingChanges(view.rules, drafts);
    const pending = Object.keys(changes).length;
    count.textContent = pending ? `${pending} ${pending === 1 ? 'change' : 'changes'} not saved` : '';
    save.disabled = !editable || pending === 0;
    discard.disabled = drafts.size === 0;

    const shown = view.rules.filter((rule) => ruleMatches(rule, search.value)
      && (!changedOnly.checked || isChanged(rule, current(rule))));
    const byGroup = new Map();
    for (const rule of shown) {
      if (!byGroup.has(rule.category)) byGroup.set(rule.category, []);
      byGroup.get(rule.category).push(rule);
    }
    groups.replaceChildren(...(shown.length === 0
      ? [h('p', { class: 'c-muted' }, view.rules.length === 0 ? '' : 'No rule matches.')]
      : [...byGroup].map(([group, rules]) => h('section', { class: 'c-gamerules__group' },
        h('h2', { class: 'c-gamerules__heading' }, GROUP_LABELS[group] ?? group),
        h('ul', { class: 'c-gamerules__list' }, rules.map((rule) => row(rule, editable)))))));
  }

  /** The value on the page: the draft if there is one. */
  function current(rule) {
    if (!drafts.has(rule.name)) return rule.value;
    const draft = drafts.get(rule.name);
    return rule.type === 'int' && /^-?\d+$/.test(String(draft).trim()) ? Number(draft) : draft;
  }

  function row(rule, editable) {
    const value = current(rule);
    const id = `gamerule-${rule.name.replace(/[^A-Za-z0-9_-]/g, '-')}`;
    const edit = (next) => {
      if (next === rule.value || String(next).trim() === String(rule.value)) drafts.delete(rule.name);
      else drafts.set(rule.name, next);
      render();
    };
    const control = rule.type === 'bool'
      ? h('input', { id, type: 'checkbox', name: rule.name, checked: value === true, disabled: !editable, onChange: (event) => edit(event.target.checked) })
      : h('input', { id, class: 'c-input c-gamerules__number', type: 'number', step: 1, name: rule.name, value: String(value), disabled: !editable, onChange: (event) => edit(event.target.value) });
    const changed = isChanged(rule, value);
    return h('li', { class: `c-gamerules__rule${drafts.has(rule.name) ? ' is-edited' : ''}`, dataset: { rule: rule.name } },
      h('div', { class: 'c-gamerules__text' },
        h('label', { class: 'c-gamerules__label', for: id }, rule.label ?? rule.name),
        h('code', { class: 'c-gamerules__name' }, rule.name),
        rule.description ? h('p', { class: 'c-muted c-gamerules__description' }, rule.description) : null),
      h('div', { class: 'c-gamerules__value' },
        changed ? h('span', { class: 'c-badge c-gamerules__changed', title: `Default: ${rule.default}` }, 'Changed') : null,
        changed && editable ? h('button', { class: 'c-button c-button--ghost c-button--small', type: 'button', 'aria-label': `Reset ${rule.label ?? rule.name}`, onClick: () => edit(rule.default) }, 'Reset') : null,
        control));
  }

  search.addEventListener('input', () => { if (view) render(); });
  changedOnly.addEventListener('change', () => { if (view) render(); });

  return { element, load, refresh };
}
