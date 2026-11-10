/**
 * A server's worlds: each folder with a level.dat, what its files say about it, and what can be done:
 * make it the one the server plays, download it, upload another, delete any (the active one too, as it
 * may be the one that won't load) or reset the active one (both back up first).
 *
 *   const worlds = createWorlds({ instanceId, can });
 *   panel.append(worlds.element);
 *   worlds.load();      // on opening the tab
 *   worlds.refresh();   // when the server starts or stops
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { dateTime, relativeTime, sizeLabel } from '../core/format.js';
import { toast } from '../core/toast.js';
import { finishedJob } from '../core/wait-job.js';

const MODES = { survival: 'Survival', creative: 'Creative', adventure: 'Adventure', spectator: 'Spectator' };

export function createWorlds({ instanceId, can }) {
  const canWrite = can('files.write');
  const note = h('p', { class: 'c-muted', role: 'status' });
  const list = h('div', { class: 'c-worlds__list' });
  const fileInput = h('input', { class: 'c-input', type: 'file', name: 'world_zip', accept: '.zip,application/zip', 'aria-label': 'World zip' });
  const folderInput = h('input', { class: 'c-input', name: 'world_folder', placeholder: 'Folder name', maxlength: 64, 'aria-label': 'Folder name' });
  const activateInput = h('input', { type: 'checkbox', name: 'world_activate' });
  const uploadButton = h('button', { class: 'c-button', type: 'submit' }, 'Upload');
  const uploadForm = canWrite ? h('form', { class: 'c-worlds__upload', onSubmit: (event) => { event.preventDefault(); upload(); } },
    h('h2', { class: 'c-worlds__heading' }, 'Add a world from a zip'),
    h('div', { class: 'c-worlds__row' }, fileInput, folderInput,
      h('label', { class: 'c-check' }, activateInput, h('span', null, 'Make it the one the server plays')),
      uploadButton)) : null;
  const element = h('section', { class: 'c-worlds' }, note, list, uploadForm);

  let view = null;
  let loaded = false;
  let latestRead = 0;

  async function load() {
    loaded = true;
    await refresh();
  }

  async function refresh() {
    if (!loaded) return;
    const thisRead = ++latestRead;
    try {
      const fresh = await api.get(`/instances/${instanceId}/worlds`);
      if (thisRead !== latestRead) return;
      view = fresh;
      render();
    } catch (error) {
      if (thisRead === latestRead) note.textContent = error.message;
    }
  }

  function render() {
    note.textContent = view.running
      ? 'The server is running: stop it to download, delete or reset a world.'
      : view.worlds.some((world) => world.active)
        ? `The server plays “${view.active}”.`
        : `The server plays “${view.active}”, which isn’t there yet: the next start makes it, unless you make another world active.`;
    list.replaceChildren(...(view.worlds.length === 0
      ? [h('p', { class: 'c-muted' }, `No world yet: the server makes “${view.active}” when it starts.`)]
      : view.worlds.map(card)));
  }

  function card(world) {
    const facts = [
      world.version ? `Minecraft ${world.version}` : null,
      MODES[world.game_mode] ?? null,
      world.difficulty ? `${world.difficulty[0].toUpperCase()}${world.difficulty.slice(1)}` : null,
      world.hardcore ? 'Hardcore' : null,
      sizeLabel(world.size_bytes),
      world.last_played ? `played ${relativeTime(world.last_played)}` : null,
    ].filter(Boolean);
    return h('article', { class: `c-card c-worlds__card${world.active ? ' is-active' : ''}`, dataset: { world: world.folder } },
      h('header', { class: 'c-worlds__card-head' },
        h('h2', { class: 'c-worlds__name' }, world.folder),
        world.active ? h('span', { class: 'c-badge' }, 'Active') : null,
        world.name && world.name !== world.folder ? h('span', { class: 'c-muted' }, `called “${world.name}” inside`) : null),
      h('p', { class: 'c-muted' }, facts.join(' · ')),
      world.problem ? h('p', { class: 'c-worlds__problem' }, `${world.problem[0].toUpperCase()}${world.problem.slice(1)}: the game may not load it.`) : null,
      h('dl', { class: 'c-details' },
        world.seed ? row('Seed', h('code', null, world.seed)) : null,
        world.parts.length ? row('Also', world.parts.join(', ')) : null,
        world.last_played ? row('Last played', dateTime(world.last_played)) : null,
        world.datapacks.enabled.length ? row('Datapacks', world.datapacks.enabled.join(', ')) : null),
      actions(world));
  }

  function actions(world) {
    const buttons = [];
    if (!world.active && can('properties.write')) {
      buttons.push(button('Make active', () => activate(world)));
    }
    if (can('files.read') && !view.running) {
      for (const folder of [world.folder, ...world.parts]) {
        buttons.push(h('a', { class: 'c-button c-button--ghost', href: `/api/v1/instances/${instanceId}/worlds/${encodeURIComponent(folder)}/download`, download: `${folder}.zip` },
          world.parts.length ? `Download ${folder}` : 'Download'));
      }
    }
    if (canWrite && !view.running) {
      // Any world can be deleted: the active one is often the one that won't load. Reset is the same for
      // the active world, with a choice of seed for the new one.
      if (world.active) buttons.push(button('Reset…', () => reset(world), 'c-button--danger-ghost'));
      buttons.push(button('Delete…', () => remove(world), 'c-button--danger-ghost'));
    }
    return buttons.length ? h('div', { class: 'c-actions' }, buttons) : null;
  }

  async function activate(world) {
    try {
      const { restart_needed: restart } = await api.post(`/instances/${instanceId}/worlds/${encodeURIComponent(world.folder)}/activate`);
      toast(restart ? `${world.folder} is played after the next restart.` : `The server now plays ${world.folder}.`, { kind: 'success' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  async function remove(world) {
    const after = world.active
      ? ` The server plays it: the next start makes a new “${world.folder}”, unless you make another world active first.`
      : '';
    const typed = window.prompt(`Type "${world.folder}" to delete this world. It is backed up first, then moved to the trash.${after}`);
    if (typed === null) return;
    if (typed.trim() !== world.folder) {
      toast('The name did not match; nothing was deleted.', { kind: 'error' });
      return;
    }
    await runJob(api.delete(`/instances/${instanceId}/worlds/${encodeURIComponent(world.folder)}`), `${world.folder} was deleted; it is in the backups.`);
  }

  async function reset(world) {
    const typed = window.prompt(`Type "${world.folder}" to reset this world. It is backed up first; the next start makes a new one.`);
    if (typed === null) return;
    if (typed.trim() !== world.folder) {
      toast('The name did not match; nothing was reset.', { kind: 'error' });
      return;
    }
    const seed = window.prompt('Seed for the new world (empty for a random one; Cancel keeps the current setting):', '');
    const body = seed === null ? {} : { seed };
    await runJob(api.post(`/instances/${instanceId}/worlds/${encodeURIComponent(world.folder)}/reset`, body), `${world.folder} was reset; the old one is in the backups.`);
  }

  async function runJob(started, success) {
    try {
      const { job_id: jobId } = await started;
      toast('Backing up first…', { kind: 'info' });
      const job = await finishedJob(jobId, () => element.isConnected);
      if (job) toast(job.status === 'succeeded' ? success : `It failed: ${job.error ?? 'unknown error'}`, { kind: job.status === 'succeeded' ? 'success' : 'error' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  async function upload() {
    const file = fileInput.files[0];
    const folder = folderInput.value.trim();
    if (!file || !folder) {
      toast('Choose a zip and a folder name.', { kind: 'error' });
      return;
    }
    uploadButton.disabled = true;
    try {
      const query = activateInput.checked ? '?activate=true' : '';
      const world = await api.upload(`/instances/${instanceId}/worlds/${encodeURIComponent(folder)}${query}`, file);
      toast(`${world.folder} was added${world.version ? ` (Minecraft ${world.version})` : ''}.`, { kind: 'success' });
      fileInput.value = '';
      folderInput.value = '';
    } catch (error) {
      toast(error.message, { kind: 'error' });
    } finally {
      uploadButton.disabled = false;
    }
    await refresh();
  }

  fileInput.addEventListener('change', () => {
    if (!folderInput.value && fileInput.files[0]) folderInput.value = fileInput.files[0].name.replace(/\.zip$/i, '').replace(/[^A-Za-z0-9 _-]/g, '_').slice(0, 64);
  });

  return { element, load, refresh };
}

function row(term, description) {
  return h('div', { class: 'c-details__row' }, h('dt', null, term), h('dd', null, description));
}

function button(label, onClick, modifier = '') {
  return h('button', { class: `c-button ${modifier}`.trim(), type: 'button', onClick }, label);
}
