/**
 * Keeping a server's mods or plugins current (part of components/content.js): which installed
 * files have a newer version that fits the server, updating the chosen ones (in a job that takes
 * a full backup first), and naming files that were added by hand so they can be updated too.
 *
 *   const updates = createContentUpdates({ instanceId, canManage, noun: 'mod' });
 *   panel.append(updates.element);
 *   updates.check();          // on opening, and whenever the installed files change
 */
import { api } from '../core/api.js';
import { describeIdentified, updateLabel } from '../core/content-plan.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

export function createContentUpdates({ instanceId, canManage, noun }) {
  const base = `/instances/${instanceId}/content`;
  const status = h('p', { class: 'c-muted c-content-updates__status' }, 'Checking for updates…');
  const checkButton = button('Check again', check, 'c-button--ghost');
  const list = h('ul', { class: 'c-content-updates__list' });
  const updateButton = canManage ? button('Update selected', update, 'c-button--primary') : null;
  const unknown = h('p', { class: 'c-muted c-content-updates__unknown', hidden: true });
  const element = h('section', { class: 'c-content-updates', 'aria-label': 'Updates' },
    h('div', { class: 'c-content-updates__bar' }, status, checkButton),
    list,
    updateButton ? h('div', { class: 'c-content-updates__actions', hidden: true },
      updateButton,
      h('span', { class: 'c-muted' }, 'Everything is backed up first.')) : null,
    unknown,
  );
  let checking = null;

  /** Checks (once at a time; a check asked for meanwhile reuses the running one). */
  function check() {
    checking ??= run().finally(() => { checking = null; });
    return checking;
  }

  async function run() {
    checkButton.disabled = true;
    try {
      render(await api.get(`${base}/updates`));
    } catch (error) {
      status.textContent = `Could not check for updates: ${error.message}`;
      list.replaceChildren();
      setActionsHidden(true);
    } finally {
      checkButton.disabled = false;
    }
  }

  function render({ updates, unknown: unknownFiles }) {
    status.textContent = updates.length === 0
      ? 'Everything from Modrinth is up to date.'
      : `${updates.length} ${updates.length === 1 ? 'update' : 'updates'} available.`;
    list.replaceChildren(...updates.map(item));
    setActionsHidden(updates.length === 0);
    renderUnknown(unknownFiles);
  }

  function item(update) {
    const label = h('span', { class: 'c-content-updates__name' }, update.name);
    const versions = h('span', { class: 'c-muted c-content-updates__versions' }, updateLabel(update));
    if (!canManage) return h('li', { class: 'c-content-updates__item' }, label, versions);
    const box = h('input', { type: 'checkbox', checked: true, value: update.file, name: 'update' });
    return h('li', { class: 'c-content-updates__item' },
      h('label', { class: 'c-content-updates__choice' }, box, label, versions));
  }

  function renderUnknown(files) {
    unknown.hidden = files.length === 0;
    if (files.length === 0) return;
    const what = files.length === 1 ? `1 ${noun} has` : `${files.length} ${noun}s have`;
    unknown.replaceChildren(`${what} no known source, so ${files.length === 1 ? 'it isn’t' : 'they aren’t'} checked. `,
      canManage ? button('Look them up on Modrinth', identify, 'c-button--ghost') : '');
  }

  function setActionsHidden(hidden) {
    if (updateButton) updateButton.parentElement.hidden = hidden;
  }

  async function update() {
    const files = [...list.querySelectorAll('input[name="update"]:checked')].map((box) => box.value);
    if (files.length === 0) {
      toast(`Choose at least one ${noun} to update.`, { kind: 'error' });
      return;
    }
    updateButton.disabled = true;
    try {
      await api.post(`${base}/update`, { files });
      toast('Backing up, then updating…', { kind: 'info' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    } finally {
      updateButton.disabled = false;
    }
  }

  async function identify(event) {
    const clicked = event.currentTarget;
    clicked.disabled = true;
    try {
      const result = await api.post(`${base}/identify`, {});
      toast(describeIdentified(result), { kind: result.identified.length ? 'success' : 'info' });
      await check();
    } catch (error) {
      toast(error.message, { kind: 'error' });
      clicked.disabled = false;
    }
  }

  function button(label, onClick, modifier) {
    return h('button', { class: `c-button ${modifier}`, type: 'button', onClick }, label);
  }

  return { element, check };
}
