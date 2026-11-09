/**
 * Settings of the app (owners only). So far the CurseForge API key: it is write-only, so this page
 * can say that one is saved but can never show it. Saving tries the key on CurseForge first.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';
import { session } from '../core/session.js';
import { toast } from '../core/toast.js';

export async function mount(root) {
  if (!session.canGlobal('settings.manage')) {
    root.append(h('section', { class: 'c-card c-empty' }, h('h1', { class: 'c-empty__title' }, 'Only owners can change settings.')));
    return () => {};
  }
  const status = h('p', { class: 'c-muted', 'data-curseforge-status': '' });
  const removeButton = h('button', { class: 'c-button c-button--danger-ghost', type: 'button', hidden: true, onClick: remove }, 'Remove the key');
  const form = h('form', { class: 'c-form', novalidate: true },
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'curseforge-key' }, 'API key'),
      h('input', { class: 'c-input', id: 'curseforge-key', name: 'key', type: 'password', autocomplete: 'off', required: true })),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' },
      h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Save'),
      removeButton),
  );
  root.append(
    h('header', { class: 'l-page-header' }, h('h1', { class: 'l-page-header__title' }, 'Settings')),
    h('section', { class: 'c-card' },
      h('h2', { class: 'c-users__heading' }, 'CurseForge'),
      h('p', { class: 'c-muted' }, 'Finding mods and plugins on CurseForge needs your own API key, from ',
        h('a', { href: 'https://console.curseforge.com/', target: '_blank', rel: 'noopener noreferrer' }, 'the CurseForge console'),
        '. It stays on this server and is only sent to CurseForge.'),
      status,
      form),
  );

  function show({ curseforge_key_set: set }) {
    status.textContent = set ? 'A key is saved. Saving another one replaces it.' : 'No key is saved, so CurseForge is off.';
    removeButton.hidden = !set;
  }

  onSubmit(form, async (values) => {
    show(await api.put('/settings/curseforge-key', { key: values.key }));
    form.reset();
    toast('CurseForge key saved.', { kind: 'success' });
  });

  async function remove() {
    if (!window.confirm('Remove the CurseForge key? Files already installed stay.')) return;
    try {
      await api.delete('/settings/curseforge-key');
      show({ curseforge_key_set: false });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  show(await api.get('/settings'));
  return () => {};
}
