/**
 * The pack a server was made from (part of components/content.js): which version it is on, whether
 * a newer one exists, and moving to it. The new version comes from the pack's source or an upload;
 * what it would change is shown and confirmed first. Nothing shows on a server without a pack.
 *
 *   const pack = createPackUpdates({ instanceId, canManage });
 *   panel.append(pack.element);
 *   pack.load();              // on opening, and after an update
 */
import { api } from '../core/api.js';
import { sourceName } from '../core/content-plan.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';
import { finishedJob } from '../core/wait-job.js';

const KEPT_REASONS = {
  changed: 'changed since the pack installed it',
  removed: 'removed since the pack installed it',
  turned_off: 'turned off',
  not_from_pack: 'didn’t come from the pack',
};

export function createPackUpdates({ instanceId, canManage }) {
  const base = `/instances/${instanceId}/modpack`;
  const status = h('p', { class: 'c-pack__status' });
  const actions = h('div', { class: 'c-pack__actions' });
  const review = h('div', { class: 'c-pack__review' });
  const element = h('section', { class: 'c-pack', 'aria-label': 'Modpack', hidden: true }, status, actions, review);

  async function load() {
    let pack;
    try {
      pack = await api.get(base);
    } catch (error) {
      element.hidden = true;
      if (error.status !== 404) toast(error.message, { kind: 'error' });
      return;
    }
    element.hidden = false;
    review.replaceChildren();
    render(pack);
  }

  function render(pack) {
    const version = pack.version ? ` ${pack.version}` : '';
    const origin = pack.source === 'file' ? 'uploaded' : `from ${sourceName(pack.source)}`;
    status.replaceChildren(
      h('strong', null, `${pack.name}${version}`),
      h('span', { class: 'c-muted' }, ` (modpack, ${origin})`),
      pack.update ? h('span', null, `. ${pack.update.name} is available.`) : '',
      !pack.update && pack.project_id && !pack.check_failed ? h('span', { class: 'c-muted' }, '. Up to date.') : '',
      pack.check_failed ? h('span', { class: 'c-muted' }, `. Could not check for a newer version: ${pack.check_failed}`) : '',
    );
    if (!canManage) {
      actions.replaceChildren();
      return;
    }
    actions.replaceChildren(
      pack.update ? h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => fetchUpdate(pack.update.version_id) }, 'Review the update') : '',
      h('label', { class: 'c-button c-button--ghost' }, 'Upload a version',
        h('input', { class: 'u-visually-hidden', type: 'file', name: 'pack_update_file', accept: '.mrpack,.zip', onChange: (event) => upload(event.currentTarget) })),
    );
  }

  const say = (message) => review.replaceChildren(h('p', { class: 'c-muted', role: 'status' }, message));
  const fail = (message) => review.replaceChildren(h('p', { class: 'c-form__error', role: 'alert' }, message));

  async function fetchUpdate(versionId) {
    say('Downloading the new version…');
    try {
      const { upload_id: uploadId, job_id: jobId } = await api.post(`${base}/fetch`, { version_id: versionId });
      const job = await finishedJob(jobId, () => element.isConnected);
      if (!job) return;
      if (job.status !== 'succeeded') {
        fail(job.error ?? 'The download failed.');
        return;
      }
      await showPlan(uploadId, false);
    } catch (error) {
      fail(error.message);
    }
  }

  async function upload(input) {
    const [file] = input.files;
    if (!file) return;
    say(`Uploading ${file.name}…`);
    try {
      const { upload_id: uploadId } = await api.upload(`${base}/upload?file=${encodeURIComponent(file.name)}`, file);
      await showPlan(uploadId, false);
    } catch (error) {
      fail(error.message);
    } finally {
      input.value = '';
    }
  }

  async function showPlan(uploadId, replaceChanged) {
    say('Comparing it with what is on the server…');
    const query = new URLSearchParams({ upload_id: uploadId, replace_changed: String(replaceChanged) });
    const plan = await api.get(`${base}/plan?${query}`);
    review.replaceChildren(planView(uploadId, plan, replaceChanged));
  }

  function planView(uploadId, plan, replaceChanged) {
    const to = plan.to ? `${plan.name} ${plan.to}` : plan.name;
    const replaceBox = h('input', { type: 'checkbox', name: 'pack_replace_changed', checked: replaceChanged, onChange: () => showPlan(uploadId, replaceBox.checked).catch((error) => fail(error.message)) });
    const applying = h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => apply(uploadId, replaceBox.checked, applying) }, 'Update now');
    return h('div', { class: 'c-pack__plan', dataset: { upload: uploadId } },
      h('h3', { class: 'c-import__title' }, `Update to ${to}`),
      h('ul', { class: 'c-import__notes' },
        plan.version_change ? h('li', null, `Moves the server to Minecraft ${plan.version_change.mc_version}${plan.version_change.loader_version ? `, loader ${plan.version_change.loader_version}` : ''}`) : null,
        plan.server_pack ? h('li', null, 'Uses the server pack its author made') : null,
        h('li', null, `${plan.add.length} files to add, ${plan.replace.length} to replace, ${plan.remove.length} to remove`),
        plan.left_out.map((note) => h('li', null, note))),
      fileList('To add', plan.add),
      fileList('To replace', plan.replace),
      fileList('To remove', plan.remove),
      plan.kept.length ? h('details', { class: 'c-pack__kept', open: true },
        h('summary', null, `${plan.kept.length} left as they are`),
        h('ul', null, plan.kept.map((kept) => h('li', null, h('code', null, kept.path), ` (${KEPT_REASONS[kept.reason] ?? kept.reason})`)))) : null,
      plan.kept.length || replaceChanged ? h('label', { class: 'c-check' }, replaceBox, h('span', null, 'Replace these with the pack’s versions too')) : null,
      h('div', { class: 'c-form__actions' }, applying, h('span', { class: 'c-muted' }, 'The server must be stopped. Everything is backed up first.')),
    );
  }

  function fileList(title, files) {
    if (files.length === 0) return null;
    return h('details', null, h('summary', null, `${title}: ${files.length}`),
      h('ul', null, files.map((file) => h('li', null, h('code', null, file)))));
  }

  async function apply(uploadId, replaceChanged, button) {
    button.disabled = true;
    try {
      await api.post(`${base}/update`, { upload_id: uploadId, replace_changed: replaceChanged });
      say('Backing up, then updating…');
    } catch (error) {
      toast(error.message, { kind: 'error' });
      button.disabled = false;
    }
  }

  return { element, load };
}
