/**
 * Visual `server.properties` editor with a raw-text view.
 *
 *   const editor = createPropertiesEditor({ instanceId, onRestart });
 *   panel.append(editor.element);
 *   await editor.load();
 *
 * The form shows every key the schema knows (grouped by category, advanced ones on request)
 * plus unknown keys from the file under "Other". Edits are collected locally and sent as one
 * change set with the revision they started from; the server refuses stale revisions.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

const CATEGORIES = [
  ['general', 'General'],
  ['world', 'World'],
  ['players', 'Players & access'],
  ['performance', 'Performance'],
  ['network', 'Network'],
  ['resource_pack', 'Resource pack'],
  ['remote_control', 'Remote control'],
  ['advanced', 'Advanced'],
];

export function createPropertiesEditor({ instanceId, onRestart }) {
  let data = null; // last server response
  let edits = new Map(); // key → new value (form view)
  let errors = new Map(); // key → message from the server
  let view = 'form';
  let showAdvanced = false;
  let query = '';

  const search = h('input', {
    class: 'c-input c-props__search', type: 'search', placeholder: 'Search settings', 'aria-label': 'Search settings',
    onInput: (event) => { query = event.target.value.trim().toLowerCase(); renderForm(); },
  });
  const advancedToggle = h('input', {
    type: 'checkbox', onChange: (event) => { showAdvanced = event.target.checked; renderForm(); },
  });
  const viewButtons = {
    form: h('button', { class: 'c-button', type: 'button', onClick: () => setView('form') }, 'Form'),
    raw: h('button', { class: 'c-button', type: 'button', onClick: () => setView('raw') }, 'Raw file'),
  };
  const notice = h('div', { class: 'c-banner is-info', role: 'status', hidden: true });
  const form = h('div', { class: 'c-props__form' });
  const raw = h('textarea', {
    class: 'c-input c-props__raw', spellcheck: 'false', 'aria-label': 'server.properties', rows: 30,
    onInput: renderFooter,
  });
  const rawErrors = h('ul', { class: 'c-props__raw-errors', hidden: true });
  const footerText = h('span', { class: 'c-props__footer-text' });
  const discard = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: discardChanges }, 'Discard');
  const save = h('button', { class: 'c-button c-button--primary', type: 'button', onClick: saveChanges }, 'Save');
  const footer = h('div', { class: 'c-props__footer', hidden: true }, footerText, discard, save);

  const element = h('section', { class: 'c-props' },
    h('div', { class: 'c-props__toolbar' },
      search,
      h('label', { class: 'c-props__toggle' }, advancedToggle, ' Show advanced'),
      h('div', { class: 'c-segmented', role: 'group', 'aria-label': 'Editor view' }, viewButtons.form, viewButtons.raw),
    ),
    notice,
    form,
    h('div', { class: 'c-props__raw-wrap', hidden: true }, raw, rawErrors),
    footer,
  );
  const rawWrap = element.querySelector('.c-props__raw-wrap');

  async function load() {
    try {
      accept(await api.get(`/instances/${instanceId}/properties`));
    } catch (error) {
      form.replaceChildren(h('p', { class: 'c-muted' }, `Could not load server.properties: ${error.message}`));
    }
  }

  function accept(response) {
    data = response;
    edits = new Map();
    errors = new Map();
    raw.value = data.text;
    raw.readOnly = !data.can_edit;
    rawErrors.hidden = true;
    renderNotice();
    renderForm();
    setView(view);
  }

  function setView(next) {
    view = next;
    form.hidden = view !== 'form';
    rawWrap.hidden = view !== 'raw';
    search.disabled = view !== 'form';
    for (const [name, button] of Object.entries(viewButtons)) button.classList.toggle('is-active', name === view);
    renderFooter();
  }

  function renderNotice() {
    const parts = [];
    if (!data.exists) parts.push('The server writes the full file on its first start; until then, defaults are shown.');
    if (!data.can_edit) parts.push('You can view these settings but not change them.');
    notice.hidden = parts.length === 0;
    notice.textContent = parts.join(' ');
  }

  // ---- form view ----

  function fileValue(key) {
    return data.values.find((entry) => entry.key === key)?.value;
  }

  function currentValue(schema) {
    return edits.has(schema.key) ? edits.get(schema.key) : (fileValue(schema.key) ?? schema.default);
  }

  function matches(schema) {
    if (!query) return true;
    return [schema.key, schema.label, schema.description].some((text) => text.toLowerCase().includes(query));
  }

  function renderForm() {
    if (!data) return;
    const sections = [];
    for (const [category, label] of CATEGORIES) {
      const fields = data.schema
        .filter((schema) => schema.category === category && (showAdvanced || !schema.advanced || errors.has(schema.key)) && matches(schema))
        .map(field);
      if (fields.length > 0) sections.push(section(label, fields));
    }
    const known = new Set(data.schema.map((schema) => schema.key));
    const unknown = data.values
      .filter((entry) => !known.has(entry.key))
      .map((entry) => ({ key: entry.key, label: entry.key, description: 'Not a standard Minecraft setting (added by a plugin or by hand).', type: 'text', default: '', unknown: true }))
      .filter(matches)
      .map(field);
    if (unknown.length > 0) sections.push(section('Other', unknown));
    form.replaceChildren(...(sections.length > 0 ? sections : [h('p', { class: 'c-muted' }, 'No settings match your search.')]));
    renderFooter();
  }

  function section(label, fields) {
    return h('section', { class: 'c-props__section' }, h('h2', { class: 'c-props__heading' }, label), ...fields);
  }

  function field(schema) {
    const value = schema.unknown ? (edits.get(schema.key) ?? fileValue(schema.key) ?? '') : currentValue(schema);
    const original = schema.unknown ? fileValue(schema.key) : (fileValue(schema.key) ?? schema.default);
    const changed = edits.has(schema.key) && edits.get(schema.key) !== original;
    const readOnly = !data.can_edit || schema.managed;
    const id = `prop-${schema.key.replace(/[^a-z0-9]+/gi, '-')}`;
    const control = controlFor(schema, value, id, readOnly);
    const error = errors.get(schema.key);
    const isDefault = !schema.unknown && value === schema.default;
    return h('div', { class: `c-props__field${changed ? ' is-changed' : ''}${error ? ' is-invalid' : ''}` },
      h('div', { class: 'c-props__label' },
        h('label', { for: id }, schema.label),
        h('code', { class: 'c-props__key' }, schema.key),
      ),
      h('div', { class: 'c-props__control' }, control),
      h('p', { class: 'c-props__help' },
        schema.description,
        schema.managed ? h('span', { class: 'c-props__note' }, ' Set by the app (the sandbox only allows this server’s own ports).') : null,
      ),
      error ? h('p', { class: 'c-props__error', role: 'alert' }, `${schema.key} ${error}`) : null,
      !schema.unknown && !readOnly && !isDefault
        ? h('button', { class: 'c-button c-button--ghost c-props__reset', type: 'button', onClick: () => change(schema, schema.default) },
          `Reset to default (${schema.default === '' ? 'empty' : schema.default})`)
        : null,
    );
  }

  function controlFor(schema, value, id, readOnly) {
    const onValue = (next) => change(schema, next);
    if (schema.type === 'bool') {
      const box = h('input', { id, type: 'checkbox', disabled: readOnly, onChange: (e) => onValue(e.target.checked ? 'true' : 'false') });
      box.checked = value === 'true';
      return h('label', { class: 'c-switch' }, box, h('span', null, value === 'true' ? 'On' : 'Off'));
    }
    if (schema.type === 'choice') {
      const select = h('select', { id, class: 'c-input', disabled: readOnly, onChange: (e) => onValue(e.target.value) },
        schema.options.map((option) => h('option', { value: option }, option)));
      if (!schema.options.includes(value)) select.append(h('option', { value }, `${value} (not a known value)`));
      select.value = value;
      return select;
    }
    const suggestions = schema.type === 'suggest' ? `${id}-options` : null;
    const input = h('input', {
      id,
      class: 'c-input',
      type: schema.type === 'int' ? 'number' : (schema.sensitive ? 'password' : 'text'),
      min: schema.min ?? null,
      max: schema.max ?? null,
      step: schema.type === 'int' ? '1' : null,
      list: suggestions,
      readOnly,
      autocomplete: 'off',
      spellcheck: 'false',
      onChange: (e) => onValue(e.target.value),
    });
    input.value = value;
    if (suggestions) {
      return h('div', { class: 'c-props__inline' }, input,
        h('datalist', { id: suggestions }, schema.options.map((option) => h('option', { value: option }))));
    }
    if (!schema.sensitive || readOnly) return input;
    const reveal = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => {
      input.type = input.type === 'password' ? 'text' : 'password';
    } }, 'Show');
    return h('div', { class: 'c-props__inline' }, input, reveal);
  }

  function change(schema, value) {
    const original = schema.unknown ? fileValue(schema.key) : (fileValue(schema.key) ?? schema.default);
    if (value === original) edits.delete(schema.key);
    else edits.set(schema.key, value);
    errors.delete(schema.key);
    renderForm();
  }

  // ---- saving ----

  function pendingCount() {
    if (!data) return 0;
    if (view === 'raw') return raw.value !== data.text ? 1 : 0;
    return edits.size;
  }

  function renderFooter() {
    const count = pendingCount();
    footer.hidden = !data?.can_edit || count === 0;
    footerText.textContent = view === 'raw' ? 'The file was edited' : `${count} unsaved change${count === 1 ? '' : 's'}`;
  }

  function discardChanges() {
    edits = new Map();
    errors = new Map();
    raw.value = data.text;
    rawErrors.hidden = true;
    renderForm();
  }

  async function saveChanges() {
    const body = view === 'raw'
      ? { revision: data.revision, text: raw.value }
      : { revision: data.revision, changes: Object.fromEntries(edits) };
    save.disabled = true;
    try {
      const response = await api.put(`/instances/${instanceId}/properties`, body);
      accept(response);
      savedToast(response.restart_required);
    } catch (error) {
      handleSaveError(error);
    } finally {
      save.disabled = false;
    }
  }

  function savedToast(restartRequired) {
    if (!restartRequired) {
      toast('Saved. The server uses the new settings next time it starts.', { kind: 'success' });
      return;
    }
    toast('Saved. Restart the server to apply the changes.', {
      kind: 'success',
      action: onRestart ? { label: 'Restart now', onClick: onRestart } : null,
    });
  }

  function handleSaveError(error) {
    if (error.code === 'invalid_properties' && Array.isArray(error.details)) {
      errors = new Map(error.details.map((detail) => [detail.key, detail.message]));
      rawErrors.replaceChildren(...error.details.map((d) => h('li', null, `${d.key} ${d.message}`)));
      rawErrors.hidden = view !== 'raw';
      renderForm();
      toast('Some values are invalid. They are marked below.', { kind: 'error' });
    } else if (error.code === 'file_changed') {
      toast('server.properties changed on disk (the server may have rewritten it). Reload to see the current file; your unsaved changes will be lost.', {
        kind: 'error',
        action: { label: 'Reload', onClick: load },
      });
    } else {
      toast(error.message, { kind: 'error' });
    }
  }

  return { element, load, hasUnsavedChanges: () => pendingCount() > 0 };
}
