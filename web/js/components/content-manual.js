/**
 * Files that wait for the user (part of components/content.js). Some CurseForge authors only allow
 * downloading by hand, so installing leaves those files out. This lists them with a link to each,
 * and takes the downloaded jars by picking or dropping. The server looks up what it was given, so
 * an uploaded file is tracked like any other.
 *
 * The list lasts until the page is left: the plan can be made again from the search.
 *
 *   const manual = createManualDownloads({ instanceId });
 *   panel.append(manual.element);
 *   manual.add(manualDownloads(plan.steps));
 */
import { api } from '../core/api.js';
import { describeIdentified } from '../core/content-plan.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

export function createManualDownloads({ instanceId }) {
  const uploadUrl = (file) => `/instances/${instanceId}/content/upload?file=${encodeURIComponent(file)}`;
  /** What is still missing, by file name. */
  const waiting = new Map();
  const list = h('ul', { class: 'c-manual__list' });
  const input = h('input', { type: 'file', multiple: true, accept: '.jar', name: 'manual_files', onChange: () => {
    uploadAll([...input.files]);
    input.value = '';
  } });
  const element = h('section', { class: 'c-manual', hidden: true, 'aria-label': 'Downloads to do by hand' },
    h('h2', { class: 'c-content__heading' }, 'Download by hand'),
    h('p', { class: 'c-muted' }, 'The authors of these files only allow downloading them from their page. Download each one, then add it here.'),
    list,
    h('label', { class: 'c-manual__drop' }, 'Choose the downloaded files, or drop them here. ', input));
  element.addEventListener('dragover', (event) => event.preventDefault());
  element.addEventListener('drop', (event) => {
    event.preventDefault();
    uploadAll([...event.dataTransfer.files]);
  });

  function add(files) {
    for (const file of files) waiting.set(file.filename, file);
    render();
  }

  function render() {
    element.hidden = waiting.size === 0;
    list.replaceChildren(...[...waiting.values()].map((file) => h('li', { class: 'c-manual__item', dataset: { file: file.filename } },
      h('span', { class: 'c-content__name' }, file.title),
      h('span', { class: 'c-muted c-content__file' }, file.filename),
      h('a', { href: file.url, target: '_blank', rel: 'noopener noreferrer' }, 'Open on CurseForge'))));
  }

  async function uploadAll(files) {
    for (const file of files) {
      try {
        toast(describeIdentified(await api.upload(uploadUrl(file.name), file)), { kind: 'success' });
        waiting.delete(file.name);
      } catch (error) {
        toast(`${file.name}: ${error.message}`, { kind: 'error' });
      }
    }
    render();
  }

  return { element, add };
}
