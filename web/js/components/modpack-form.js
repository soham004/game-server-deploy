/**
 * What a modpack needs, and the form that creates a server for it. The same for an uploaded pack and
 * one fetched from a source: both are waiting under an upload id by now.
 *
 *   status.replaceChildren(modpackForm(uploaded));
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';
import { navigate } from '../core/navigation.js';

export function modpackForm({ upload_id: uploadId, pack, server_pack: serverPack }) {
  const version = pack.loader_version ? ` ${pack.loader_version}` : '';
  const form = h('form', { class: 'c-form', novalidate: true, dataset: { pack: uploadId } },
    h('h3', { class: 'c-import__title' }, pack.version ? `${pack.name} ${pack.version}` : pack.name),
    h('ul', { class: 'c-import__notes' },
      h('li', null, `Minecraft ${pack.mc_version}, ${pack.loader}${version}`),
      h('li', null, serverPack ? 'Uses the server pack its author made' : `${pack.files} files to download`)),
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'pack-name' }, 'Server name'),
      h('input', { class: 'c-input', id: 'pack-name', name: 'name', maxlength: 64, value: pack.name })),
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'pack-memory' }, 'Maximum memory (MB)'),
      h('input', { class: 'c-input', id: 'pack-memory', name: 'max_memory_mb', type: 'number', min: 512, max: 65536, step: 256, value: 4096 })),
    h('label', { class: 'c-check' }, h('input', { type: 'checkbox', name: 'eula_accepted', value: 'yes' }),
      h('span', null, 'I accept the ', h('a', { href: 'https://aka.ms/MinecraftEULA', target: '_blank', rel: 'noopener noreferrer' }, 'Minecraft EULA'), '.')),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' }, h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Create the server')),
  );
  onSubmit(form, async (values) => {
    const { instance } = await api.post('/modpacks/install', {
      upload_id: uploadId,
      name: values.name,
      max_memory_mb: Number(values.max_memory_mb),
      eula_accepted: values.eula_accepted === 'yes',
    });
    navigate(`/servers/${instance.id}`);
  });
  return form;
}
