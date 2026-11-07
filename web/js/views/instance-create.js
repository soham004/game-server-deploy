/** "New server": name, software (version picker), memory, EULA. */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';
import { memoryLabel } from '../core/format.js';
import { navigate } from '../core/navigation.js';
import { createVersionPicker } from '../components/version-picker.js';

const DEFAULT_MEMORY_MB = 2048;

export function mount(root) {
  let selection = { mc_version: null, loader: 'vanilla', loader_version: null };
  const picker = createVersionPicker({ onChange: (value) => { selection = value; } });

  const memoryOutput = h('output', { class: 'c-field__value', for: 'create-memory' }, memoryLabel(DEFAULT_MEMORY_MB));
  const memory = h('input', {
    class: 'c-range', id: 'create-memory', name: 'max_memory_mb', type: 'range',
    min: '512', max: '16384', step: '512', value: String(DEFAULT_MEMORY_MB),
  });
  memory.addEventListener('input', () => { memoryOutput.textContent = memoryLabel(Number(memory.value)); });

  const form = h('form', { class: 'c-form c-create', novalidate: true },
    h('div', { class: 'c-field' },
      h('label', { class: 'c-field__label', for: 'create-name' }, 'Server name'),
      h('input', { class: 'c-input', id: 'create-name', name: 'name', maxlength: '64', required: true, autocomplete: 'off', placeholder: 'Survival with friends' }),
    ),
    h('fieldset', { class: 'c-fieldset' },
      h('legend', { class: 'c-field__label' }, 'Software'),
      picker.element,
    ),
    h('div', { class: 'c-field' },
      h('label', { class: 'c-field__label', for: 'create-memory' }, 'Maximum memory ', memoryOutput),
      memory,
      h('p', { class: 'c-field__hint' }, '2–4 GB suits most small servers; modpacks often need 6 GB or more.'),
    ),
    h('label', { class: 'c-check' },
      h('input', { type: 'checkbox', name: 'eula_accepted', value: 'yes' }),
      h('span', null,
        'I accept the ',
        h('a', { href: 'https://aka.ms/MinecraftEULA', target: '_blank', rel: 'noopener noreferrer' }, 'Minecraft End User License Agreement'),
        '.',
      ),
    ),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' },
      h('a', { class: 'c-button c-button--ghost', href: '/' }, 'Cancel'),
      h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Create server'),
    ),
  );

  onSubmit(form, async (values) => {
    if (!selection.mc_version) throw new Error('Pick a Minecraft version.');
    const { instance } = await api.post('/instances', {
      name: values.name,
      mc_version: selection.mc_version,
      loader: selection.loader,
      loader_version: selection.loader_version,
      max_memory_mb: Number(values.max_memory_mb),
      eula_accepted: values.eula_accepted === 'yes',
    });
    navigate(`/servers/${instance.id}`);
  });

  root.append(
    h('header', { class: 'l-page-header' }, h('h1', { class: 'l-page-header__title' }, 'New server')),
    h('section', { class: 'c-card' }, form),
  );
}
