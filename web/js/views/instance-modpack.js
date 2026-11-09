/**
 * "New server from a modpack": search Modrinth (and CurseForge, with a key), pick a pack, and it is
 * downloaded in a job. A pack then shows what it needs and the form that creates its server. An
 * archive that turns out to be a plain server folder continues on the import page.
 */
import { modpackForm } from '../components/modpack-form.js';
import { api } from '../core/api.js';
import { sourceName } from '../core/content-plan.js';
import { debounce } from '../core/debounce.js';
import { h } from '../core/dom.js';
import { relativeTime } from '../core/format.js';
import { navigate } from '../core/navigation.js';
import { finishedJob } from '../core/wait-job.js';

const TYPING_PAUSE_MS = 200;
const PAGE_SIZE = 20;
const SORTS = [
  ['downloads', 'Most downloaded'],
  ['relevance', 'Best match'],
  ['updated', 'Recently updated'],
  ['newest', 'Newest'],
];

export async function mount(root) {
  const results = h('ul', { class: 'c-content-browser__results' });
  const chosen = h('div', { class: 'c-import__upload-result' });
  const text = h('input', { class: 'c-input', type: 'search', name: 'pack_query', placeholder: 'Search modpacks', 'aria-label': 'Search modpacks' });
  const source = h('select', { class: 'c-input', name: 'pack_source', 'aria-label': 'Where to search' });
  const sort = h('select', { class: 'c-input', name: 'pack_sort', 'aria-label': 'Sort by' },
    SORTS.map(([value, label]) => h('option', { value }, label)));
  const version = h('select', { class: 'c-input', name: 'pack_mc', 'aria-label': 'Minecraft version' },
    h('option', { value: '' }, 'Any Minecraft version'));
  const more = h('button', { class: 'c-button c-button--ghost', type: 'button', hidden: true, onClick: () => search({ append: true }) }, 'Show more');

  root.append(
    h('header', { class: 'l-page-header' },
      h('h1', { class: 'l-page-header__title' }, 'New server from a modpack'),
      h('a', { class: 'c-button c-button--ghost', href: '/servers/new' }, 'Back'),
    ),
    chosen,
    h('section', { class: 'c-content-browser' },
      h('form', { class: 'c-content-browser__search', onSubmit: (event) => { event.preventDefault(); search(); } }, source, text, sort, version),
      results, more),
  );

  let offset = 0;
  /** Numbers each search, so an older one that answers late can't replace newer results. */
  let latestSearch = 0;

  async function search({ append = false } = {}) {
    const thisSearch = ++latestSearch;
    const nextOffset = append ? offset + PAGE_SIZE : 0;
    const query = new URLSearchParams({ source: source.value, q: text.value.trim(), sort: sort.value, mc: version.value, offset: String(nextOffset) });
    try {
      const page = await api.get(`/modpacks/search?${query}`);
      if (thisSearch !== latestSearch) return;
      offset = nextOffset;
      const rows = page.hits.map(row);
      if (append) results.append(...rows);
      else results.replaceChildren(...(rows.length ? rows : [h('li', { class: 'c-muted' }, 'No modpacks found.')]));
      more.hidden = offset + page.hits.length >= page.total;
    } catch (error) {
      if (thisSearch !== latestSearch) return;
      results.replaceChildren(h('li', { class: 'c-muted' }, `Search failed: ${error.message}`));
      more.hidden = true;
    }
  }

  function row(hit) {
    const actions = h('div', { class: 'c-content-browser__actions' },
      h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => fetchPack(hit) }, 'Use this pack'),
      h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: (event) => chooseVersion(hit, event.currentTarget) }, 'Other versions'));
    return h('li', { class: 'c-content-result', dataset: { project: hit.id } },
      h('span', { class: 'c-content-result__pick' }), // the row's first column: nothing to tick here
      hit.icon_url ? h('img', { class: 'c-content-result__icon', src: hit.icon_url, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' }) : h('span', { class: 'c-content-result__icon' }),
      h('div', { class: 'c-content-result__text' },
        h('a', { class: 'c-content-result__title', href: hit.page_url, target: '_blank', rel: 'noopener noreferrer' }, hit.title),
        h('span', { class: 'c-muted' }, ` by ${hit.author}`),
        h('p', { class: 'c-content-result__description' }, hit.description),
        h('p', { class: 'c-muted c-content-result__meta' },
          `${hit.downloads.toLocaleString()} downloads${hit.updated ? ` · updated ${relativeTime(hit.updated)}` : ''}`)),
      actions);
  }

  async function chooseVersion(hit, button) {
    button.disabled = true;
    try {
      const all = await api.get(`/modpacks/projects/${encodeURIComponent(hit.id)}/versions?source=${hit.source}`);
      const versions = version.value ? all.filter((v) => v.game_versions.includes(version.value)) : all;
      if (versions.length === 0) {
        chosen.replaceChildren(failure(`${hit.title} has no versions to download.`));
        button.disabled = false;
        return;
      }
      const select = h('select', { class: 'c-input', 'aria-label': `Version of ${hit.title}` },
        versions.map((v) => h('option', { value: v.id }, `${v.version_number}${v.channel === 'release' ? '' : ` (${v.channel})`}`)));
      button.replaceWith(h('div', { class: 'c-content-result__versions' }, select,
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => fetchPack(hit, select.value) }, 'Use this version')));
    } catch (error) {
      chosen.replaceChildren(failure(error.message));
      button.disabled = false;
    }
  }

  /** Downloads the pack in a job and follows it, then shows the form (or moves on to the import). */
  async function fetchPack(hit, versionId = undefined) {
    chosen.replaceChildren(h('p', { class: 'c-muted', role: 'status' }, `Downloading ${hit.title}…`));
    chosen.scrollIntoView({ block: 'nearest' });
    try {
      const { upload_id: uploadId, job_id: jobId } = await api.post('/modpacks/fetch', { source: hit.source, project_id: hit.id, version_id: versionId, mc_version: version.value });
      const job = await finishedJob(jobId, () => chosen.isConnected);
      if (!job) return; // left the page
      if (job.status !== 'succeeded') {
        chosen.replaceChildren(failure(job.error ?? `The download ${job.status === 'cancelled' ? 'was cancelled' : 'failed'}.`));
        return;
      }
      const fetched = await api.get(`/modpacks/uploads/${uploadId}`);
      if (fetched.kind === 'server_folder') navigate('/servers/import');
      else chosen.replaceChildren(h('section', { class: 'c-card c-import__card' }, modpackForm(fetched)));
    } catch (error) {
      chosen.replaceChildren(failure(error.message));
    }
  }

  const failure = (message) => h('p', { class: 'c-form__error', role: 'alert' }, message);

  const searchSoon = debounce(() => search(), TYPING_PAUSE_MS);
  text.addEventListener('input', () => searchSoon());
  for (const control of [source, sort, version]) control.addEventListener('change', () => search());

  try {
    const [{ sources }, versions] = await Promise.all([api.get('/modpacks/sources'), api.get('/catalog/minecraft/versions')]);
    source.replaceChildren(...sources.map((value) => h('option', { value }, sourceName(value))));
    source.hidden = sources.length < 2;
    version.append(...versions.filter((v) => v.kind === 'release').map((v) => h('option', { value: v.id }, v.id)));
  } catch (error) {
    results.replaceChildren(h('li', { class: 'c-muted' }, error.message));
    return;
  }
  await search();
}
