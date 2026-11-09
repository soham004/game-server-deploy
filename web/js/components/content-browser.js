/**
 * Finding and installing mods or plugins, pre-filtered to the server (its loader, its Minecraft
 * version, server-side). Searching starts by itself once typing stops. A picker chooses the source
 * when the server has more than one (plugin servers: Modrinth or Hangar).
 *
 * A result can be installed on its own (Install, or Other versions to pick one), or ticked: it
 * then moves to the "To be installed" list (components/content-install-list.js, `installList`,
 * placed by the tab), which stays across searches and installs everything on it together. Either way the plan (every required
 * dependency, what gets replaced) is shown and confirmed first. Projects the server already has
 * say "Installed" instead of Install (updates are in the updates panel); Other versions stays, to
 * go back to an older one.
 *
 *   const browser = createContentBrowser({ instanceId, canInstall, noun: () => 'mod' });
 *   panel.append(browser.installList, browser.element);
 *   browser.setSources(list.sources);
 *   browser.setInstalled(installedProjects(list.items));   // whenever the installed list loads
 *   browser.search();
 */
import { api } from '../core/api.js';
import { debounce } from '../core/debounce.js';
import { h } from '../core/dom.js';
import { countLabel, describePlan, manualDownloads, projectKey, sourceName } from '../core/content-plan.js';
import { relativeTime } from '../core/format.js';
import { toast } from '../core/toast.js';
import { createInstallList } from './content-install-list.js';

/** How long typing must pause before searching. */
const TYPING_PAUSE_MS = 200;

const SORTS = [
  ['relevance', 'Best match'],
  ['downloads', 'Most downloaded'],
  ['updated', 'Recently updated'],
  ['newest', 'Newest'],
];

/**
 * @param {{ instanceId: string, canInstall: boolean, noun: () => string, onManual: (files: object[]) => void }} options
 *   `noun`: "mod" or "plugin", for the server as it is now. `onManual`: files an install left for
 *   the user to download by hand.
 */
export function createContentBrowser({ instanceId, canInstall, noun, onManual }) {
  const base = `/instances/${instanceId}/content`;
  let offset = 0;
  let total = 0;
  /** Numbers each search, so an older one that answers late can't replace newer results. */
  let latestSearch = 0;
  /** `projectKey`s of what the server has, and of what is being installed right now. */
  let installed = new Set();
  const installing = new Set();
  /** The listed results by `projectKey`: the hit and the parts of its row that change. */
  const shown = new Map();

  const text = h('input', { class: 'c-input', type: 'search', name: 'content_query' });
  const source = h('select', { class: 'c-input', name: 'content_source', 'aria-label': 'Where to search', hidden: true });
  const sort = h('select', { class: 'c-input', name: 'content_sort', 'aria-label': 'Sort by' },
    SORTS.map(([value, label]) => h('option', { value }, label)));
  const searchSoon = debounce(() => search(), TYPING_PAUSE_MS);
  const searchNow = () => {
    searchSoon.cancel();
    search();
  };
  // Enter still searches at once (the form has one text field, so Enter submits it).
  const form = h('form', { class: 'c-content-browser__search', onSubmit: (event) => { event.preventDefault(); searchNow(); } },
    source, text, sort);
  text.addEventListener('input', () => searchSoon());
  sort.addEventListener('change', searchNow);
  source.addEventListener('change', () => {
    describeSearch();
    searchNow();
  });
  const installList = createInstallList({
    noun,
    // Unticked (or installed): back among the results, if it is still listed there.
    onRemove: drawRow,
    onInstall: (hits) => planAndInstall(hits, hits.map((hit) => request(hit))),
  });
  const results = h('ul', { class: 'c-content-browser__results' });
  const more = h('button', { class: 'c-button c-button--ghost', type: 'button', hidden: true, onClick: () => search({ append: true }) }, 'Show more');
  const element = h('section', { class: 'c-content-browser' }, form, results, more);
  setSources(['modrinth']);

  /** The sources this server can use; the picker only shows when there is a choice. */
  function setSources(sources) {
    const chosen = source.value;
    source.replaceChildren(...sources.map((value) => h('option', { value }, sourceName(value))));
    if (sources.includes(chosen)) source.value = chosen;
    source.hidden = sources.length < 2;
    describeSearch();
  }

  function describeSearch() {
    const label = `Search ${sourceName(source.value)}`;
    text.placeholder = label;
    text.setAttribute('aria-label', label);
  }

  /** What the API needs to install a result (at its best version, or the one given). */
  function request(hit, versionId = undefined) {
    return { source: hit.source, project_id: hit.id, version_id: versionId };
  }

  async function search({ append = false } = {}) {
    const thisSearch = ++latestSearch;
    const nextOffset = append ? offset + 20 : 0;
    const query = new URLSearchParams({ source: source.value, q: text.value.trim(), sort: sort.value, offset: String(nextOffset) });
    try {
      const page = await api.get(`${base}/search?${query}`);
      if (thisSearch !== latestSearch) return; // a newer search is on its way
      offset = nextOffset;
      total = page.total;
      if (!append) shown.clear();
      const items = page.hits.map(result);
      if (append) results.append(...items);
      else results.replaceChildren(...(items.length ? items : [h('li', { class: 'c-muted' }, 'Nothing found for this server.')]));
      more.hidden = offset + page.hits.length >= total;
    } catch (error) {
      if (thisSearch !== latestSearch) return;
      results.replaceChildren(h('li', { class: 'c-muted' }, `Search failed: ${error.message}`));
      more.hidden = true;
    }
  }

  /** What the server has now; results redraw to match. Ends any "Installing…" too: this is called after an install finishes. */
  function setInstalled(keys) {
    installed = keys;
    installing.clear();
    for (const key of keys) installList.remove(key); // installed some other way meanwhile
    for (const key of shown.keys()) drawRow(key);
  }

  function result(hit) {
    const key = projectKey(hit.source, hit.id);
    const row = h('li', { class: 'c-content-result', dataset: { project: hit.id } });
    const pick = h('span', { class: 'c-content-result__pick' });
    const actions = h('div', { class: 'c-content-browser__actions' });
    shown.set(key, { hit, row, pick, actions });
    row.append(
      pick,
      hit.icon_url ? h('img', { class: 'c-content-result__icon', src: hit.icon_url, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' }) : h('span', { class: 'c-content-result__icon' }),
      h('div', { class: 'c-content-result__text' },
        h('a', { class: 'c-content-result__title', href: hit.page_url, target: '_blank', rel: 'noopener noreferrer' }, hit.title),
        h('span', { class: 'c-muted' }, ` by ${hit.author}`),
        h('p', { class: 'c-content-result__description' }, hit.description),
        h('p', { class: 'c-muted c-content-result__meta' },
          `${hit.downloads.toLocaleString()} downloads${hit.updated ? ` · updated ${relativeTime(hit.updated)}` : ''}`,
          hit.client_side === 'unsupported' ? ' · server-only (players don’t need it)' : ''),
      ),
      actions,
    );
    drawRow(key);
    return row;
  }

  /**
   * A result's checkbox and buttons for its state. Ticked ones are on the install list instead
   * (hidden here). Installable: a checkbox, Install and Other versions. Installed: a disabled
   * "Installed" and Other versions (going back to an older version replaces the installed file).
   */
  function drawRow(key) {
    const listed = shown.get(key);
    if (!listed) return;
    const { hit, row, pick, actions } = listed;
    row.hidden = installList.has(key);
    const disabled = (label) => h('button', { class: 'c-button', type: 'button', disabled: true }, label);
    const otherVersions = () => h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: (event) => chooseVersion(hit, event.currentTarget) }, 'Other versions');
    const installable = canInstall && !installing.has(key) && !installed.has(key);
    pick.replaceChildren(...(installable ? [h('label', null,
      h('input', { type: 'checkbox', onChange: () => { installList.add(key, hit); drawRow(key); } }),
      h('span', { class: 'u-visually-hidden' }, `Add ${hit.title} to the install list`))] : []));
    if (installing.has(key)) {
      actions.replaceChildren(disabled('Installing…'));
    } else if (installed.has(key)) {
      actions.replaceChildren(disabled('Installed'), ...(canInstall ? [otherVersions()] : []));
    } else if (canInstall) {
      actions.replaceChildren(
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => planAndInstall([hit], [request(hit)]) }, 'Install'),
        otherVersions(),
      );
    } else {
      actions.replaceChildren();
    }
  }

  async function chooseVersion(hit, button) {
    button.disabled = true;
    try {
      const versions = await api.get(`${base}/projects/${hit.id}/versions?source=${hit.source}`);
      if (versions.length === 0) {
        toast(`${hit.title} has no version for this server.`, { kind: 'error' });
        return;
      }
      const select = h('select', { class: 'c-input', 'aria-label': `Version of ${hit.title}` },
        versions.map((v) => h('option', { value: v.id }, `${v.version_number}${v.channel === 'release' ? '' : ` (${v.channel})`}`)));
      const row = h('div', { class: 'c-content-result__versions' }, select,
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => planAndInstall([hit], [request(hit, select.value)]) }, 'Install this version'));
      button.replaceWith(row);
    } catch (error) {
      toast(error.message, { kind: 'error' });
      button.disabled = false;
    }
  }

  /**
   * Shows the plan for `projects` (the requests for `hits`), then installs when confirmed, in one
   * job. Resolves true once the install has started.
   */
  async function planAndInstall(hits, projects) {
    let plan;
    try {
      plan = await api.post(`${base}/plan`, { projects });
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return false;
    }
    if (!window.confirm(describePlan(plan))) return false;
    const byHand = manualDownloads(plan.steps);
    if (byHand.length === plan.steps.length) {
      // Nothing to download from here: no job, and no "Installed" for what isn't.
      onManual(byHand);
      return true;
    }
    try {
      await api.post(`${base}/install`, { projects });
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return false;
    }
    toast(`Installing ${hits.length === 1 ? hits[0].title : countLabel(hits.length, noun())}…`, { kind: 'info' });
    onManual(byHand);
    // Until the installed list reloads (`setInstalled`), so nothing can be started twice.
    for (const hit of hits) installing.add(projectKey(hit.source, hit.id));
    for (const hit of hits) drawRow(projectKey(hit.source, hit.id));
    return true;
  }

  // The list goes where the tab puts it (between the installed list and the search).
  return { element, installList: installList.element, search, setInstalled, setSources };
}
