/**
 * File manager for one server: browse folders, edit text files, upload, download, rename,
 * copy, compress, extract and delete (to the server's trash).
 *
 *   const browser = createFileBrowser({ instanceId, canWrite });
 *   panel.append(browser.element);
 *   await browser.load();
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { relativeTime, sizeLabel } from '../core/format.js';
import { toast } from '../core/toast.js';
import { createFileEditor } from './file-editor.js';

/** Files the server rewrites while running: edit them through the app's own tools instead. */
const LIVE_FILES = new Set(['ops.json', 'whitelist.json', 'banned-players.json', 'banned-ips.json', 'usercache.json']);
const ARCHIVE = /\.(zip|tar|tar\.gz|tgz)$/i;

export function createFileBrowser({ instanceId, canWrite }) {
  const base = `/instances/${instanceId}/files`;
  let path = '';
  let listing = null;
  let selected = new Set();

  const crumbs = h('nav', { class: 'c-files__crumbs', 'aria-label': 'Folder' });
  const fileInput = h('input', { type: 'file', multiple: true, hidden: true, 'data-upload-input': '', onChange: (e) => uploadFiles([...e.target.files]) });
  const toolbar = h('div', { class: 'c-files__toolbar' },
    crumbs,
    h('div', { class: 'c-actions' },
      canWrite ? h('button', { class: 'c-button', type: 'button', onClick: newFolder }, 'New folder') : null,
      canWrite ? h('button', { class: 'c-button', type: 'button', onClick: newFile }, 'New file') : null,
      canWrite ? h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => fileInput.click() }, 'Upload') : null,
      h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => open(path) }, 'Refresh'),
    ),
    fileInput,
  );
  const notice = h('div', { class: 'c-banner is-info', role: 'status', hidden: true });
  const selectionBar = h('div', { class: 'c-files__selection', hidden: true });
  const progress = h('div', { class: 'c-files__progress', hidden: true });
  const table = h('table', { class: 'c-files__table' });
  const dropZone = h('div', { class: 'c-files__drop' }, table);
  const editor = createFileEditor({
    base,
    canWrite,
    notice: (path) => (LIVE_FILES.has(path.split('/').pop()) && listing?.running ? 'The running server rewrites this file; prefer the Players tools.' : ''),
    onSaved: () => open(path),
  });
  const element = h('section', { class: 'c-files' }, toolbar, notice, selectionBar, progress, dropZone, editor.element);

  if (canWrite) {
    dropZone.addEventListener('dragover', (event) => { event.preventDefault(); dropZone.classList.add('is-dragging'); });
    dropZone.addEventListener('dragleave', () => dropZone.classList.remove('is-dragging'));
    dropZone.addEventListener('drop', (event) => {
      event.preventDefault();
      dropZone.classList.remove('is-dragging');
      uploadFiles([...event.dataTransfer.files]);
    });
  }

  async function open(folder) {
    try {
      listing = await api.get(`${base}?path=${encodeURIComponent(folder)}`);
      path = listing.path;
      selected = new Set();
      render();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  function render() {
    renderCrumbs();
    notice.hidden = !listing.running;
    notice.textContent = 'The server is running. It may overwrite files it uses (worlds, ops.json, …), and most changes take effect after a restart.';
    const rows = listing.entries.map(row);
    if (path !== '') rows.unshift(parentRow());
    table.replaceChildren(
      h('thead', null, h('tr', null,
        canWrite ? h('th', { class: 'c-files__check' }, '') : null,
        h('th', null, 'Name'), h('th', { class: 'c-files__num' }, 'Size'), h('th', null, 'Modified'), h('th', { class: 'c-files__actions' }, ''),
      )),
      h('tbody', null, rows.length > 0 ? rows : h('tr', null, h('td', { colspan: 5, class: 'c-muted' }, 'This folder is empty.'))),
    );
    renderSelection();
  }

  function renderCrumbs() {
    const parts = path === '' ? [] : path.split('/');
    const links = [h('button', { class: 'c-files__crumb', type: 'button', onClick: () => open('') }, 'server')];
    parts.forEach((part, index) => {
      const target = parts.slice(0, index + 1).join('/');
      links.push(h('span', { class: 'c-files__sep', 'aria-hidden': 'true' }, '/'));
      links.push(h('button', { class: 'c-files__crumb', type: 'button', onClick: () => open(target) }, part));
    });
    crumbs.replaceChildren(...links);
  }

  function parentRow() {
    const parent = path.split('/').slice(0, -1).join('/');
    return h('tr', { class: 'c-files__row is-parent' },
      canWrite ? h('td', null) : null,
      h('td', null, h('button', { class: 'c-files__name', type: 'button', onClick: () => open(parent) }, '← ..')),
      h('td'), h('td'), h('td'),
    );
  }

  function row(entry) {
    const isDir = entry.kind === 'directory';
    const check = canWrite ? h('input', {
      type: 'checkbox', 'aria-label': `Select ${entry.name}`,
      onChange: (e) => { if (e.target.checked) selected.add(entry.path); else selected.delete(entry.path); renderSelection(); },
    }) : null;
    const label = `${isDir ? '📁' : entry.kind === 'symlink' ? '🔗' : '📄'} ${entry.name}`;
    const name = h('button', {
      class: 'c-files__name', type: 'button', title: entry.path,
      onClick: () => (isDir ? open(entry.path) : openFile(entry)),
    }, label);
    return h('tr', { class: `c-files__row${LIVE_FILES.has(entry.name) ? ' is-live' : ''}`, dataset: { path: entry.path } },
      canWrite ? h('td', { class: 'c-files__check' }, check) : null,
      h('td', null, name),
      h('td', { class: 'c-files__num' }, entry.size === null ? '' : sizeLabel(entry.size)),
      h('td', { class: 'c-muted' }, entry.modified ? relativeTime(entry.modified) : ''),
      h('td', { class: 'c-files__actions' }, rowActions(entry)),
    );
  }

  function rowActions(entry) {
    const actions = [h('a', { class: 'c-button c-button--ghost', href: downloadUrl(entry.path), download: '' }, 'Download')];
    if (canWrite) {
      if (ARCHIVE.test(entry.name) && entry.kind === 'file') actions.push(action('Extract', () => extract(entry)));
      actions.push(action('Rename', () => rename(entry)));
      actions.push(action('Copy', () => copy(entry)));
      actions.push(action('Delete', () => remove([entry.path]), 'c-button--danger-ghost'));
    }
    return h('div', { class: 'c-files__row-actions' }, actions);
  }

  function action(label, onClick, modifier = '') {
    return h('button', { class: `c-button c-button--ghost ${modifier}`.trim(), type: 'button', onClick }, label);
  }

  function renderSelection() {
    selectionBar.hidden = selected.size === 0;
    selectionBar.replaceChildren(
      h('span', null, `${selected.size} selected`),
      h('button', { class: 'c-button', type: 'button', onClick: compressSelection }, 'Compress to .zip'),
      h('button', { class: 'c-button c-button--danger', type: 'button', onClick: () => remove([...selected]) }, 'Delete'),
    );
  }

  function downloadUrl(target) {
    return `/api/v1${base}/download?path=${encodeURIComponent(target)}`;
  }

  function child(name) {
    return path === '' ? name : `${path}/${name}`;
  }

  // ---- operations ----

  async function run(promise, success) {
    try {
      await promise;
      if (success) toast(success, { kind: 'success' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await open(path);
  }

  function newFolder() {
    const name = window.prompt('Folder name');
    if (name) run(api.post(`${base}/folder`, { path: child(name) }));
  }

  async function newFile() {
    const name = window.prompt('File name');
    if (!name) return;
    try {
      const file = await api.put(`${base}/text`, { path: child(name), content: '' });
      await open(path);
      editor.show(file);
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  function rename(entry) {
    const name = window.prompt(`Rename ${entry.name} to`, entry.name);
    if (name && name !== entry.name) run(api.post(`${base}/rename`, { from: entry.path, to: child(name) }));
  }

  function copy(entry) {
    const name = window.prompt(`Copy ${entry.name} as`, `${entry.name} (copy)`);
    if (name) run(api.post(`${base}/copy`, { from: entry.path, to: child(name) }));
  }

  function remove(paths) {
    const what = paths.length === 1 ? paths[0] : `${paths.length} items`;
    if (!window.confirm(`Delete ${what}? It is moved to the server's trash.`)) return;
    run(api.post(`${base}/delete`, { paths }), `Deleted ${what}.`);
  }

  function extract(entry) {
    const stem = entry.name.replace(ARCHIVE, '');
    const destination = window.prompt(`Extract ${entry.name} into the folder`, child(stem));
    if (destination === null) return;
    run(api.post(`${base}/extract`, { path: entry.path, destination }).then((summary) => {
      toast(`Extracted ${summary.files} ${summary.files === 1 ? 'file' : 'files'}${summary.skipped ? ` (${summary.skipped} unsafe or unsupported entries skipped)` : ''}.`, { kind: 'success' });
    }));
  }

  function compressSelection() {
    const name = window.prompt('Archive name', 'archive.zip');
    if (name) run(api.post(`${base}/compress`, { paths: [...selected], destination: child(name) }), `Created ${name}.`);
  }

  async function uploadFiles(files) {
    fileInput.value = '';
    for (const file of files) {
      try {
        await uploadOne(file);
      } catch (error) {
        if (error.code === 'file_exists' && window.confirm(`${file.name} already exists. Replace it?`)) {
          await uploadOne(file, true).catch((e) => toast(e.message, { kind: 'error' }));
        } else if (error.code !== 'file_exists') {
          toast(`${file.name}: ${error.message}`, { kind: 'error' });
        }
      }
    }
    progress.hidden = true;
    await open(path);
  }

  /** XMLHttpRequest, because fetch can't report upload progress. */
  function uploadOne(file, overwrite = false) {
    return new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.open('PUT', `/api/v1${base}/upload?path=${encodeURIComponent(child(file.name))}&overwrite=${overwrite}`);
      request.setRequestHeader('X-CSRF', '1');
      request.upload.addEventListener('progress', (event) => {
        progress.hidden = false;
        const percent = event.lengthComputable ? Math.round((event.loaded / event.total) * 100) : 0;
        progress.textContent = `Uploading ${file.name}… ${percent}%`;
      });
      request.addEventListener('load', () => {
        if (request.status >= 200 && request.status < 300) return resolve();
        let error = {};
        try { error = JSON.parse(request.responseText).error ?? {}; } catch { /* not JSON */ }
        reject(Object.assign(new Error(error.message ?? `Upload failed (HTTP ${request.status}).`), { code: error.code }));
      });
      request.addEventListener('error', () => reject(new Error('The upload was interrupted.')));
      request.send(file);
    });
  }

  async function openFile(entry) {
    try {
      editor.show(await api.get(`${base}/text?path=${encodeURIComponent(entry.path)}`));
    } catch (error) {
      if (error.code === 'not_text' || error.code === 'file_too_large') {
        toast(`${error.message}`, { kind: 'info', action: { label: 'Download', onClick: () => window.location.assign(downloadUrl(entry.path)) } });
      } else {
        toast(error.message, { kind: 'error' });
      }
    }
  }

  return { element, load: () => open('') };
}
