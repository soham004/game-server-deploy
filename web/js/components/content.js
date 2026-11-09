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
import { installedProjects, sourceName } from '../core/content-plan.js';
import { h } from '../core/dom.js';
import { sizeLabel } from '../core/format.js';
import { toast } from '../core/toast.js';
import { live } from '../core/ws.js';
import { createContentBrowser } from './content-browser.js';
import { createManualDownloads } from './content-manual.js';
import { createPackUpdates } from './content-modpack.js';
import { createContentUpdates } from './content-updates.js';

export function createContent({ instanceId, can, isRunning }) {
  const base = `/instances/${instanceId}/content`;
  const canManage = can('content.manage');
  let list = null;
  const notice = h('p', { class: 'c-banner is-info', hidden: true }, 'Restart the server to load the changes.');
  const summary = h('p', { class: 'c-muted c-content__summary' });
  const installed = h('div', { class: 'c-content__installed' });
  const manual = createManualDownloads({ instanceId });
  const pack = createPackUpdates({ instanceId, canManage });
  const browser = createContentBrowser({ instanceId, canInstall: canManage, noun: () => (list?.kind === 'plugins' ? 'plugin' : 'mod'), onManual: manual.add });
  let updates = null;
  const updatesSlot = h('div', { class: 'c-content__updates' });
  const element = h('section', { class: 'c-content' },
    notice,
    pack.element,
    h('h2', { class: 'c-content__heading' }, 'Installed'), summary, updatesSlot, installed,
    manual.element,
    browser.installList,
    h('h2', { class: 'c-content__heading' }, 'Find more'), browser.element);
  let changedWhileRunning = false;
  let searched = false;

  async function load() {
    try {
      list = await api.get(base);
      browser.setSources(list.sources);
      browser.setInstalled(installedProjects(list.items));
      render();
      checkUpdates();
      if (!searched) {
        searched = true;
        browser.search();
        pack.load();
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
      updates = createContentUpdates({ instanceId, canManage, noun: list.kind === 'plugins' ? 'plugin' : 'mod', onManual: manual.add });
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
    const source = sourceCell(item.source);
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

  /** Where a file came from: a link to its page when known. */
  function sourceCell(origin) {
    if (!origin) return h('span', { class: 'c-muted', title: 'Added by hand or from another source' }, 'Unknown');
    const name = sourceName(origin.source);
    return origin.page_url ? h('a', { href: origin.page_url, target: '_blank', rel: 'noopener noreferrer' }, name) : h('span', null, name);
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
    if (event.type !== 'job' || job.instance_id !== instanceId) return;
    if (job.kind === 'modpack.update' && job.status !== 'running') {
      if (job.status === 'succeeded') toast('The pack is updated.', { kind: 'success' });
      else toast(job.error ?? 'The update did not finish.', { kind: 'error' });
      pack.load();
      load();
    }
    if (job.status !== 'succeeded') return;
    if (job.kind === 'content.install') toast('Installed.', { kind: 'success' });
    if (job.kind === 'content.update') toast('Updated.', { kind: 'success' });
  });

  function destroy() {
    offInstances();
    offJobs();
  }

  return { element, load, destroy };
}
