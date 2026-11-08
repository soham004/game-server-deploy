/**
 * A server's backups: back up now, then a list to restore, pin, annotate, download or delete.
 *
 *   const backups = createBackups({ instanceId, can, software: () => instance.software });
 *   panel.append(backups.element);
 *   await backups.load();
 *
 * Backing up and restoring run as jobs; the list reloads when the server reports a change
 * (`backups_changed`), so every open page stays current.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { dateTime, relativeTime, sizeLabel } from '../core/format.js';
import { toast } from '../core/toast.js';
import { live } from '../core/ws.js';

const KIND_LABELS = {
  manual: 'Manual',
  scheduled: 'Scheduled',
  pre_restore: 'Before a restore',
  pre_version_change: 'Before a version change',
};

const SCOPE_LABELS = {
  world: 'Worlds',
  full: 'Everything',
};

/**
 * @param {{ instanceId: string, can: (permission: string) => boolean, software: () => string }} options
 *   `software`: what the server runs now, to warn when a backup was taken on something else.
 */
export function createBackups({ instanceId, can, software }) {
  let data = null;
  const summary = h('p', { class: 'c-muted c-backups__summary' });
  const list = h('div', { class: 'c-backups__list' });
  const element = h('section', { class: 'c-backups' },
    can('backups.create') ? createForm() : null,
    summary,
    list,
  );
  const url = `/instances/${instanceId}/backups`;

  async function load() {
    try {
      data = await api.get(url);
      render();
    } catch (error) {
      list.replaceChildren(h('p', { class: 'c-muted' }, `Could not load the backups: ${error.message}`));
    }
  }

  function createForm() {
    const scope = h('select', { class: 'c-input', name: 'scope', id: 'backup-scope', 'aria-label': 'What to back up' },
      h('option', { value: 'world' }, 'Worlds only'),
      h('option', { value: 'full' }, 'Everything (worlds, configuration, mods and plugins)'));
    const note = h('input', { class: 'c-input', name: 'note', id: 'backup-note', maxlength: 200, placeholder: 'Note (optional)', 'aria-label': 'Note' });
    const button = h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Back up now');
    const form = h('form', { class: 'c-backups__create', onSubmit: async (event) => {
      event.preventDefault();
      button.disabled = true;
      try {
        await api.post(url, { scope: scope.value, note: note.value });
        note.value = '';
        toast('Backing up…', { kind: 'info' });
      } catch (error) {
        toast(error.message, { kind: 'error' });
      } finally {
        button.disabled = false;
      }
    } }, scope, note, button);
    return form;
  }

  function render() {
    const { backups, policy, total_bytes: total } = data;
    summary.textContent = backups.length === 0
      ? `No backups yet. The newest ${policy.keep} are kept; pinned ones are never deleted automatically.`
      : `${backups.length} ${backups.length === 1 ? 'backup' : 'backups'}, ${sizeLabel(total)} in total. `
        + `The newest ${policy.keep} are kept; pinned ones are never deleted automatically.`;
    if (backups.length === 0) {
      list.replaceChildren();
      return;
    }
    list.replaceChildren(h('table', { class: 'c-backups__table' },
      h('thead', null, h('tr', null,
        h('th', { scope: 'col' }, 'Taken'),
        h('th', { scope: 'col' }, 'Contents'),
        h('th', { scope: 'col', class: 'c-backups__num' }, 'Size'),
        h('th', { scope: 'col' }, 'Note'),
        h('th', { scope: 'col' }, h('span', { class: 'u-visually-hidden' }, 'Actions')),
      )),
      h('tbody', null, backups.map(row)),
    ));
  }

  function row(backup) {
    return h('tr', { dataset: { backup: backup.id }, class: backup.pinned ? 'is-pinned' : null },
      h('td', null,
        h('span', { class: 'c-backups__date' }, dateTime(backup.created_at)),
        h('span', { class: 'c-muted c-backups__ago' }, relativeTime(backup.created_at)),
      ),
      h('td', null,
        SCOPE_LABELS[backup.scope] ?? backup.scope,
        backup.kind === 'manual' ? null : h('span', { class: 'c-badge c-backups__kind' }, KIND_LABELS[backup.kind] ?? backup.kind),
        backup.software ? h('span', { class: 'c-muted c-backups__software' }, backup.software) : null,
      ),
      h('td', { class: 'c-backups__num' }, sizeLabel(backup.size_bytes)),
      h('td', { class: 'c-backups__note' }, backup.note ?? ''),
      h('td', { class: 'c-backups__actions' }, h('div', { class: 'c-backups__row-actions' }, actions(backup))),
    );
  }

  function actions(backup) {
    const buttons = [];
    if (can('backups.create')) {
      buttons.push(button(backup.pinned ? 'Unpin' : 'Pin', () => change(backup, { pinned: !backup.pinned }),
        backup.pinned ? 'Let retention delete this backup again' : 'Never delete this backup automatically'));
      buttons.push(button('Note', () => editNote(backup)));
    }
    if (can('backups.restore')) {
      buttons.push(h('a', { class: 'c-button c-button--ghost', href: `/api/v1${url}/${backup.id}/download`, download: '' }, 'Download'));
      buttons.push(button('Restore', () => restore(backup)));
      buttons.push(button('Delete', () => remove(backup), null, 'c-button--danger-ghost'));
    }
    return buttons;
  }

  function button(label, onClick, title = null, modifier = 'c-button--ghost') {
    return h('button', { class: `c-button ${modifier}`, type: 'button', title, onClick }, label);
  }

  async function change(backup, body) {
    try {
      await api.patch(`${url}/${backup.id}`, body);
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function editNote(backup) {
    const note = window.prompt('Note for this backup (leave empty to remove it)', backup.note ?? '');
    if (note === null) return;
    await change(backup, { note });
  }

  async function restore(backup) {
    const when = dateTime(backup.created_at);
    const what = backup.scope === 'world'
      ? 'The worlds are replaced; everything else stays as it is.'
      : 'The server folder is replaced, except logs and downloaded libraries.';
    const now = software();
    const mismatch = backup.software && now && backup.software !== now
      ? `\n\nThis backup was taken on ${backup.software}; the server now runs ${now}. `
        + 'Worlds from a newer version may not load on an older one.'
      : '';
    const confirmed = window.confirm(
      `Restore the backup from ${when}?\n\n${what} The server is stopped first and started again afterwards `
      + `if it was running. A backup of the current files is taken first, so you can undo this.${mismatch}`,
    );
    if (!confirmed) return;
    try {
      await api.post(`${url}/${backup.id}/restore`, { safety_backup: true });
      toast('Restoring…', { kind: 'info' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function remove(backup) {
    if (!window.confirm(`Delete the backup from ${dateTime(backup.created_at)}? This can't be undone.`)) return;
    try {
      await api.delete(`${url}/${backup.id}`);
      toast('Backup deleted.', { kind: 'success' });
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  const offInstances = live.on('instances', (event) => {
    if (event.instance_id === instanceId && event.type === 'backups_changed' && data) load();
  });
  const offJobs = live.on('jobs', (event) => {
    const job = event.data;
    if (event.type !== 'job' || job.instance_id !== instanceId || !job.kind.startsWith('backup.')) return;
    if (job.status === 'succeeded') {
      toast(job.kind === 'backup.restore' ? 'The backup was restored.' : 'Backup finished.', { kind: 'success' });
    }
  });

  function destroy() {
    offInstances();
    offJobs();
  }

  return { element, load, destroy };
}
