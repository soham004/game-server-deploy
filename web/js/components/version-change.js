/**
 * Changing a server's Minecraft version or loader version (Settings tab).
 *
 *   const version = createVersionChange({ instanceId, instance: () => instance });
 *   panel.append(version.element);
 *   await version.load();
 *   version.update(); // when the server's state changes
 *
 * The server takes a full backup first and installs the new version over the old files. Going
 * to an older Minecraft version can break the world, so it needs the server's name typed.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

const ADVICE = 'A full backup is taken first. Worlds are converted when the new version starts and can\'t be '
  + 'opened by older versions afterwards; mods and plugins must support the new version.';

export function createVersionChange({ instanceId, instance }) {
  let versions = [];
  const element = h('section', { class: 'c-fieldset c-version-change' });
  const mc = h('select', { class: 'c-input', name: 'target_mc', 'aria-label': 'Minecraft version' });
  const loaderVersion = h('select', { class: 'c-input', name: 'target_loader_version', 'aria-label': 'Loader version', hidden: true });
  const button = h('button', { class: 'c-button', type: 'button', onClick: change }, 'Change version');
  const summary = h('p');
  const hint = h('p', { class: 'c-field__hint' });
  mc.addEventListener('change', fillLoaderVersions);

  async function load() {
    try {
      const all = await api.get('/catalog/minecraft/versions');
      const current = instance().server.mc_version;
      versions = all.filter((version) => version.kind === 'release' || version.id === current);
    } catch (error) {
      element.replaceChildren(h('p', { class: 'c-muted' }, `Could not load the Minecraft versions: ${error.message}`));
      return;
    }
    const server = instance();
    mc.replaceChildren(...versions.map((version) => h('option', { value: version.id }, version.id)));
    mc.value = server.server.mc_version;
    element.replaceChildren(
      h('h2', { class: 'c-version-change__heading' }, 'Version'),
      summary,
      h('div', { class: 'c-version-change__row' }, mc, loaderVersion, button),
      hint,
    );
    update();
    await fillLoaderVersions();
  }

  /** Follows the server: what it runs, and whether it is stopped (needed to change). */
  function update() {
    const server = instance();
    const build = server.server.loader_version ? ` (${server.server.loader_version})` : '';
    summary.textContent = `This server runs ${server.software}${build}.`;
    const busy = server.busy || ['starting', 'running', 'stopping'].includes(server.state.status);
    button.disabled = busy;
    hint.textContent = busy ? 'Stop the server to change its version.' : ADVICE;
  }

  async function fillLoaderVersions() {
    const server = instance();
    const loader = server.server.loader;
    if (loader === 'vanilla') return;
    try {
      const list = await api.get(`/catalog/minecraft/loaders/${loader}/versions?mc=${encodeURIComponent(mc.value)}`);
      loaderVersion.replaceChildren(...list.map((version) => h('option', { value: version.id },
        `${version.id}${version.recommended ? ' (recommended)' : ''}${version.stable ? '' : ' (unstable)'}`)));
      const keep = mc.value === server.server.mc_version ? server.server.loader_version : null;
      const preferred = list.find((version) => version.id === keep) ?? list.find((version) => version.recommended) ?? list[0];
      if (preferred) loaderVersion.value = preferred.id;
      loaderVersion.hidden = list.length === 0;
    } catch (error) {
      loaderVersion.hidden = true;
      toast(`No ${loader} build for Minecraft ${mc.value}: ${error.message}`, { kind: 'error' });
    }
  }

  /** Newest first: a later position is an older version. */
  function isOlder(candidate, reference) {
    const position = (id) => versions.findIndex((version) => version.id === id);
    return position(candidate) > position(reference);
  }

  async function change() {
    const server = instance();
    const target = loaderVersion.hidden ? null : loaderVersion.value;
    const downgrade = isOlder(mc.value, server.server.mc_version);
    if (downgrade) {
      const typed = window.prompt(
        `Going back to Minecraft ${mc.value} can corrupt worlds made with ${server.server.mc_version}. `
        + `A full backup is taken first.\n\nType "${server.name}" to change the version anyway.`,
      );
      if (typed === null) return;
      if (typed.trim() !== server.name) {
        toast('The name did not match; nothing was changed.', { kind: 'error' });
        return;
      }
    } else if (!window.confirm(`Change ${server.name} to Minecraft ${mc.value}${target ? ` (${target})` : ''}? A full backup is taken first.`)) {
      return;
    }
    button.disabled = true;
    try {
      await api.post(`/instances/${instanceId}/change-version`, {
        mc_version: mc.value,
        loader_version: target,
        allow_downgrade: downgrade,
      });
      toast('Changing the version…', { kind: 'info' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
      update();
    }
  }

  return { element, load, update };
}
