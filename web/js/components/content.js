/**
 * A server's mods or plugins: what is installed (with on/off and remove), keeping it current
 * (components/content-updates.js), and finding more (components/content-browser.js). Changes need a restart to take effect, which the tab says
 * while the server runs.
 *
 *   const content = createContent({ instanceId, can, isRunning: () => … });
 *   panel.append(content.element);
 *   await content.load();
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { sizeLabel } from '../core/format.js';
import { toast } from '../core/toast.js';
import { live } from '../core/ws.js';
import { createContentBrowser } from './content-browser.js';
import { createContentUpdates } from './content-updates.js';

export function createContent({ instanceId, can, isRunning }) {
  const base = `/instances/${instanceId}/content`;
  const canManage = can('content.manage');
  let list = null;
  const notice = h('p', { class: 'c-banner is-info', hidden: true }, 'Restart the server to load the changes.');
  const summary = h('p', { class: 'c-muted c-content__summary' });
  const installed = h('div', { class: 'c-content__installed' });
  const browser = createContentBrowser({ instanceId, canInstall: canManage });
  let updates = null;
  const updatesSlot = h('div', { class: 'c-content__updates' });
  const element = h('section', { class: 'c-content' },
    notice,
    h('h2', { class: 'c-content__heading' }, 'Installed'), summary, updatesSlot, installed,
    h('h2', { class: 'c-content__heading' }, 'Find more'), browser.element);
  let changedWhileRunning = false;
  let searched = false;

  async function load() {
    try {
      list = await api.get(base);
      render();
      checkUpdates();
      if (!searched) {
        searched = true;
        browser.search();
      }
    } catch (error) {
      installed.replaceChildren(h('p', { class: 'c-muted' }, error.message));
    }
  }

  /** The updates panel needs the list's kind for its wording, so it is made on the first load. */
  function checkUpdates() {
    if (list.items.length === 0) {
      updatesSlot.replaceChildren();
      updates = null;
      return;
    }
    if (!updates) {
      updates = createContentUpdates({ instanceId, canManage, noun: list.kind === 'plugins' ? 'plugin' : 'mod' });
      updatesSlot.replaceChildren(updates.element);
    }
    updates.check();
  }

  function render() {
    const noun = list.kind === 'plugins' ? 'plugin' : 'mod';
    const enabled = list.items.filter((item) => item.enabled).length;
    summary.textContent = list.items.length === 0
      ? `No ${noun}s yet. They go in the ${list.folder} folder; find some below.`
      : `${list.items.length} ${noun}${list.items.length === 1 ? '' : 's'}, ${enabled} turned on. For Minecraft ${list.target.mc_version} (${list.target.loaders.join(', ')}).`;
    notice.hidden = !(changedWhileRunning && isRunning());
    if (list.items.length === 0) {
      installed.replaceChildren();
      return;
    }
    installed.replaceChildren(h('table', { class: 'c-content__table' },
      h('thead', null, h('tr', null,
        h('th', { scope: 'col' }, 'Name'),
        h('th', { scope: 'col' }, 'Version'),
        h('th', { scope: 'col' }, 'From'),
        h('th', { scope: 'col', class: 'c-content__num' }, 'Size'),
        h('th', { scope: 'col' }, h('span', { class: 'u-visually-hidden' }, 'Actions')),
      )),
      h('tbody', null, list.items.map(row)),
    ));
  }

  function row(item) {
    const source = item.source
      ? h('a', { href: `https://modrinth.com/project/${item.source.project_id}`, target: '_blank', rel: 'noopener noreferrer' }, 'Modrinth')
      : h('span', { class: 'c-muted', title: 'Added by hand or from another source' }, 'Unknown');
    return h('tr', { class: item.enabled ? null : 'is-disabled', dataset: { file: item.file } },
      h('td', null, h('span', { class: 'c-content__name' }, item.name), h('span', { class: 'c-muted c-content__file' }, item.file)),
      h('td', null, item.version ?? ''),
      h('td', null, source),
      h('td', { class: 'c-content__num' }, sizeLabel(item.size)),
      h('td', { class: 'c-content__actions' }, canManage ? h('div', { class: 'c-content__row-actions' },
        h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => toggle(item) }, item.enabled ? 'Turn off' : 'Turn on'),
        h('button', { class: 'c-button c-button--danger-ghost', type: 'button', onClick: () => remove(item) }, 'Remove'),
      ) : null),
    );
  }

  async function toggle(item) {
    try {
      await api.post(`${base}/files/${encodeURIComponent(item.file)}/${item.enabled ? 'disable' : 'enable'}`);
      changedWhileRunning = true;
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function remove(item) {
    if (!window.confirm(`Remove ${item.name}? The file goes to the server's trash.`)) return;
    try {
      await api.delete(`${base}/files/${encodeURIComponent(item.file)}`);
      changedWhileRunning = true;
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  const offInstances = live.on('instances', (event) => {
    if (event.instance_id !== instanceId || !list) return;
    if (event.type === 'content_changed') {
      changedWhileRunning = changedWhileRunning || isRunning();
      load();
    } else if (event.type === 'state') {
      // A restart loads the changes.
      if (event.data.status === 'starting') changedWhileRunning = false;
      render();
    }
  });
  const offJobs = live.on('jobs', (event) => {
    const job = event.data;
    if (event.type !== 'job' || job.instance_id !== instanceId || job.status !== 'succeeded') return;
    if (job.kind === 'content.install') toast('Installed.', { kind: 'success' });
    if (job.kind === 'content.update') toast('Updated.', { kind: 'success' });
  });

  function destroy() {
    offInstances();
    offJobs();
  }

  return { element, load, destroy };
}
