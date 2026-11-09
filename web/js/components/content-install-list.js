/**
 * The "To be installed" list of the Mods / Plugins tab: projects ticked in the search results
 * (components/content-browser.js) wait here, across searches, and are installed together: one
 * plan (every required dependency, each once) and one job. It looks like the installed list
 * (components/content.js): the same table, with Remove to take a project off again.
 *
 *   const list = createInstallList({ noun: () => 'mod', onRemove: (key) => …, onInstall: (hits) => … });
 *   panel.append(list.element);
 *   list.add(key, hit);   // a ticked result; `key` is its projectKey
 */
import { h } from '../core/dom.js';
import { countLabel, sourceName } from '../core/content-plan.js';

/**
 * @param {{
 *   noun: () => string,
 *   onRemove: (key: string) => void,
 *   onInstall: (hits: object[]) => Promise<boolean>,
 * }} options
 *   `noun`: "mod" or "plugin". `onRemove`: a project left the list (removed or cleared).
 *   `onInstall`: install these; resolves true when it started (the list then empties).
 */
export function createInstallList({ noun, onRemove, onInstall }) {
  /** Ticked projects in the order they were ticked, by projectKey. */
  const picked = new Map();
  const rows = h('tbody');
  const installButton = h('button', { class: 'c-button c-button--primary', type: 'button', onClick: install });
  const element = h('section', { class: 'c-install-list', hidden: true, 'aria-label': 'To be installed' },
    h('h2', { class: 'c-content__heading' }, 'To be installed'),
    h('div', { class: 'c-content__installed' }, h('table', { class: 'c-content__table' },
      h('thead', null, h('tr', null,
        h('th', { scope: 'col' }, 'Name'),
        h('th', { scope: 'col' }, 'Version'),
        h('th', { scope: 'col' }, 'From'),
        h('th', { scope: 'col' }, h('span', { class: 'u-visually-hidden' }, 'Actions')),
      )),
      rows,
    )),
    h('div', { class: 'c-install-list__actions' },
      installButton,
      h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: clear }, 'Clear'),
      h('span', { class: 'c-muted' }, 'Required dependencies come too.')),
  );

  function add(key, hit) {
    picked.set(key, hit);
    render();
  }

  function remove(key) {
    if (!picked.delete(key)) return;
    render();
    onRemove(key);
  }

  function clear() {
    for (const key of [...picked.keys()]) remove(key);
  }

  function render() {
    element.hidden = picked.size === 0;
    installButton.textContent = `Install ${countLabel(picked.size, noun())}`;
    rows.replaceChildren(...[...picked].map(([key, hit]) => row(key, hit)));
  }

  function row(key, hit) {
    return h('tr', { dataset: { project: hit.id } },
      h('td', null,
        h('span', { class: 'c-content__name' }, hit.title),
        h('span', { class: 'c-muted c-content__file' }, `by ${hit.author}`)),
      h('td', { class: 'c-muted' }, 'Newest for this server'),
      h('td', null, h('a', { href: hit.page_url, target: '_blank', rel: 'noopener noreferrer' }, sourceName(hit.source))),
      h('td', { class: 'c-content__actions' }, h('div', { class: 'c-content__row-actions' },
        h('button', { class: 'c-button c-button--danger-ghost', type: 'button', onClick: () => remove(key) }, 'Remove'))),
    );
  }

  async function install() {
    installButton.disabled = true;
    try {
      // The list empties once the install has started; the results show "Installing…" for these.
      if (await onInstall([...picked.values()])) {
        const keys = [...picked.keys()];
        picked.clear();
        render();
        keys.forEach(onRemove);
      }
    } finally {
      installButton.disabled = false;
    }
  }

  return { element, add, remove, has: (key) => picked.has(key) };
}
