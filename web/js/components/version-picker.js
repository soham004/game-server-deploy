/**
 * Two-pane version picker (modelled on Prism Launcher's "Custom" dialog):
 *   1. Minecraft version: filter by type, search, pick one.
 *   2. Server software (Vanilla / Paper / Fabric), disabled where unsupported.
 *   3. Software version, with the recommended one starred and preselected.
 *
 *   const picker = createVersionPicker({ onChange: (value) => ... });
 *   form.append(picker.element);
 *   picker.value(); // { mc_version, loader, loader_version }
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { shortDate } from '../core/format.js';

const KIND_LABELS = { release: 'Release', snapshot: 'Snapshot', old_beta: 'Beta', old_alpha: 'Alpha' };
const MAX_ROWS = 300;

export function createVersionPicker({ onChange = () => {} } = {}) {
  const state = {
    versions: [],
    filters: new Set(['release']),
    search: '',
    mcVersion: null,
    loaders: [],
    loader: 'vanilla',
    loaderVersions: [],
    loaderVersion: null,
  };

  const versionList = listbox('Minecraft versions');
  const loaderGroup = h('div', { class: 'c-picker__loaders', role: 'radiogroup', 'aria-label': 'Server software' });
  const loaderVersionList = listbox('Software versions');
  const loaderVersionSection = h('section', { class: 'c-picker__pane', hidden: true },
    h('h3', { class: 'c-picker__heading' }, 'Software version'),
    loaderVersionList,
  );
  const status = h('p', { class: 'c-picker__status', role: 'status' });

  const filterBoxes = Object.entries(KIND_LABELS).map(([kind, label]) => {
    const box = h('input', { type: 'checkbox', value: kind });
    box.checked = state.filters.has(kind);
    box.addEventListener('change', () => {
      if (box.checked) state.filters.add(kind);
      else state.filters.delete(kind);
      renderVersions();
    });
    return h('label', { class: 'c-picker__filter' }, box, ` ${label}s`);
  });
  const searchBox = h('input', { class: 'c-input c-picker__search', type: 'search', placeholder: 'Search versions', 'aria-label': 'Search versions' });
  searchBox.addEventListener('input', () => {
    state.search = searchBox.value.trim().toLowerCase();
    renderVersions();
  });

  const element = h('div', { class: 'c-picker' },
    h('section', { class: 'c-picker__pane c-picker__pane--versions' },
      h('h3', { class: 'c-picker__heading' }, 'Minecraft version'),
      h('div', { class: 'c-picker__toolbar' }, searchBox, h('div', { class: 'c-picker__filters' }, filterBoxes)),
      versionList,
    ),
    h('div', { class: 'c-picker__side' },
      h('section', { class: 'c-picker__pane' },
        h('h3', { class: 'c-picker__heading' }, 'Server software'),
        loaderGroup,
      ),
      loaderVersionSection,
    ),
    status,
  );

  function emit() {
    onChange(value());
  }

  function value() {
    return {
      mc_version: state.mcVersion,
      loader: state.loader,
      loader_version: state.loader === 'vanilla' ? null : state.loaderVersion,
    };
  }

  function renderVersions() {
    const visible = state.versions
      .filter((v) => state.filters.has(v.kind))
      .filter((v) => !state.search || v.id.toLowerCase().includes(state.search))
      .slice(0, MAX_ROWS);
    versionList.replaceChildren(...visible.map((v) => option(v.id, [v.id, shortDate(v.released), KIND_LABELS[v.kind]], v.id === state.mcVersion, () => selectVersion(v.id))));
    if (visible.length === 0) versionList.append(h('p', { class: 'c-picker__empty' }, 'No versions match.'));
  }

  async function selectVersion(id) {
    state.mcVersion = id;
    renderVersions();
    status.textContent = '';
    try {
      state.loaders = await api.get(`/catalog/minecraft/loaders?mc=${encodeURIComponent(id)}`);
    } catch (error) {
      status.textContent = error.message;
      state.loaders = [];
    }
    const current = state.loaders.find((l) => l.loader === state.loader);
    if (!current?.available) state.loader = 'vanilla';
    renderLoaders();
    await loadLoaderVersions();
  }

  function renderLoaders() {
    loaderGroup.replaceChildren(...state.loaders.map((entry) => {
      const radio = h('input', { type: 'radio', name: 'picker-loader', value: entry.loader, disabled: !entry.available });
      radio.checked = entry.loader === state.loader;
      radio.addEventListener('change', async () => {
        state.loader = entry.loader;
        await loadLoaderVersions();
      });
      return h('label', { class: `c-picker__loader${entry.available ? '' : ' is-disabled'}` },
        radio,
        h('span', { class: 'c-picker__loader-name' }, entry.name),
        entry.available ? null : h('span', { class: 'c-picker__loader-note' }, 'Not available'),
      );
    }));
  }

  async function loadLoaderVersions() {
    state.loaderVersions = [];
    state.loaderVersion = null;
    const hasVersions = state.loader !== 'vanilla';
    loaderVersionSection.hidden = !hasVersions;
    if (hasVersions && state.mcVersion) {
      loaderVersionList.replaceChildren(h('p', { class: 'c-picker__empty' }, 'Loading…'));
      try {
        state.loaderVersions = await api.get(`/catalog/minecraft/loaders/${state.loader}/versions?mc=${encodeURIComponent(state.mcVersion)}`);
      } catch (error) {
        status.textContent = error.message;
      }
      state.loaderVersion = state.loaderVersions.find((v) => v.recommended)?.id ?? state.loaderVersions[0]?.id ?? null;
    }
    renderLoaderVersions();
    emit();
  }

  function renderLoaderVersions() {
    loaderVersionList.replaceChildren(...state.loaderVersions.map((v) => {
      const label = v.recommended ? `★ ${v.id}` : v.id;
      const kind = v.stable ? 'Stable' : 'Experimental';
      return option(v.id, [label, shortDate(v.released), kind], v.id === state.loaderVersion, () => {
        state.loaderVersion = v.id;
        renderLoaderVersions();
        emit();
      });
    }));
  }

  async function load() {
    versionList.replaceChildren(h('p', { class: 'c-picker__empty' }, 'Loading versions…'));
    try {
      state.versions = await api.get('/catalog/minecraft/versions');
    } catch (error) {
      versionList.replaceChildren(h('p', { class: 'c-picker__empty' }, error.message));
      return;
    }
    const latestRelease = state.versions.find((v) => v.kind === 'release');
    renderVersions();
    if (latestRelease) await selectVersion(latestRelease.id);
  }

  load();
  return { element, value };
}

function listbox(label) {
  const list = h('div', { class: 'c-picker__list', role: 'listbox', 'aria-label': label, tabindex: '0' });
  list.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const options = [...list.querySelectorAll('[role="option"]')];
    const index = options.findIndex((o) => o.getAttribute('aria-selected') === 'true');
    const next = options[Math.max(0, Math.min(options.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
    next?.click();
    next?.scrollIntoView({ block: 'nearest' });
  });
  return list;
}

function option(id, cells, selected, onSelect) {
  return h('div', {
    class: `c-picker__row${selected ? ' is-selected' : ''}`,
    role: 'option',
    'aria-selected': String(selected),
    dataset: { id },
    onClick: onSelect,
  }, cells.map((text) => h('span', { class: 'c-picker__cell' }, text ?? '')));
}
