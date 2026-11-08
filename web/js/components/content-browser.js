/**
 * Finding and installing mods or plugins from Modrinth, pre-filtered to the server (its loader,
 * its Minecraft version, server-side). Installing first shows the plan (dependencies, what gets
 * replaced) and asks before anything changes.
 *
 *   const browser = createContentBrowser({ instanceId, canInstall });
 *   panel.append(browser.element);
 *   browser.search();
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { describePlan } from '../core/content-plan.js';
import { relativeTime } from '../core/format.js';
import { toast } from '../core/toast.js';

const SORTS = [
  ['relevance', 'Best match'],
  ['downloads', 'Most downloaded'],
  ['updated', 'Recently updated'],
  ['newest', 'Newest'],
];

export function createContentBrowser({ instanceId, canInstall }) {
  const base = `/instances/${instanceId}/content`;
  let offset = 0;
  let total = 0;
  const text = h('input', { class: 'c-input', type: 'search', name: 'content_query', placeholder: 'Search Modrinth', 'aria-label': 'Search Modrinth' });
  const sort = h('select', { class: 'c-input', name: 'content_sort', 'aria-label': 'Sort by' },
    SORTS.map(([value, label]) => h('option', { value }, label)));
  const form = h('form', { class: 'c-content-browser__search', onSubmit: (event) => { event.preventDefault(); search(); } },
    text, sort, h('button', { class: 'c-button', type: 'submit' }, 'Search'));
  sort.addEventListener('change', () => search());
  const results = h('ul', { class: 'c-content-browser__results' });
  const more = h('button', { class: 'c-button c-button--ghost', type: 'button', hidden: true, onClick: () => search({ append: true }) }, 'Show more');
  const element = h('section', { class: 'c-content-browser' }, form, results, more);

  async function search({ append = false } = {}) {
    offset = append ? offset + 20 : 0;
    const query = new URLSearchParams({ q: text.value, sort: sort.value, offset: String(offset) });
    try {
      const page = await api.get(`${base}/search?${query}`);
      total = page.total;
      const items = page.hits.map(result);
      if (append) results.append(...items);
      else results.replaceChildren(...(items.length ? items : [h('li', { class: 'c-muted' }, 'Nothing found for this server.')]));
      more.hidden = offset + page.hits.length >= total;
    } catch (error) {
      results.replaceChildren(h('li', { class: 'c-muted' }, `Search failed: ${error.message}`));
    }
  }

  function result(hit) {
    const actions = h('div', { class: 'c-content-browser__actions' });
    if (canInstall) {
      actions.append(
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => install(hit, null) }, 'Install'),
        h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: (event) => chooseVersion(hit, event.currentTarget) }, 'Other versions'),
      );
    }
    return h('li', { class: 'c-content-result', dataset: { project: hit.id } },
      hit.icon_url ? h('img', { class: 'c-content-result__icon', src: hit.icon_url, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' }) : h('span', { class: 'c-content-result__icon' }),
      h('div', { class: 'c-content-result__text' },
        h('a', { class: 'c-content-result__title', href: `https://modrinth.com/project/${hit.slug}`, target: '_blank', rel: 'noopener noreferrer' }, hit.title),
        h('span', { class: 'c-muted' }, ` by ${hit.author}`),
        h('p', { class: 'c-content-result__description' }, hit.description),
        h('p', { class: 'c-muted c-content-result__meta' },
          `${hit.downloads.toLocaleString()} downloads${hit.updated ? ` · updated ${relativeTime(hit.updated)}` : ''}`,
          hit.client_side === 'unsupported' ? ' · server-only (players don’t need it)' : ''),
      ),
      actions,
    );
  }

  async function chooseVersion(hit, button) {
    button.disabled = true;
    try {
      const versions = await api.get(`${base}/projects/${hit.id}/versions`);
      if (versions.length === 0) {
        toast(`${hit.title} has no version for this server.`, { kind: 'error' });
        return;
      }
      const select = h('select', { class: 'c-input', 'aria-label': `Version of ${hit.title}` },
        versions.map((v) => h('option', { value: v.id }, `${v.version_number}${v.channel === 'release' ? '' : ` (${v.channel})`}`)));
      const row = h('div', { class: 'c-content-result__versions' }, select,
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => install(hit, select.value) }, 'Install this version'));
      button.replaceWith(row);
    } catch (error) {
      toast(error.message, { kind: 'error' });
      button.disabled = false;
    }
  }

  /** Shows the plan, then installs when confirmed. */
  async function install(hit, versionId) {
    const body = { project_id: hit.id, version_id: versionId };
    let plan;
    try {
      plan = await api.post(`${base}/plan`, body);
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return;
    }
    if (!window.confirm(describePlan(plan))) return;
    try {
      await api.post(`${base}/install`, body);
      toast(`Installing ${hit.title}…`, { kind: 'info' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  return { element, search };
}
