/**
 * Instance settings form: name, memory, Java runtime and tuning, crash/stop behaviour, and how
 * many backups to keep.
 *
 *   const settings = createInstanceSettings({ instanceId, onSaved, onRestart });
 *   panel.append(settings.element);
 *   await settings.load();
 *
 * Only changed fields are sent; the server validates everything again.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

export function createInstanceSettings({ instanceId, onSaved, onRestart }) {
  let data = null;
  const form = h('form', { class: 'c-form c-settings', novalidate: true, onSubmit: save });
  const element = h('section', { class: 'c-settings-panel' }, form);

  async function load() {
    try {
      data = await api.get(`/instances/${instanceId}/settings`);
      render();
    } catch (error) {
      form.replaceChildren(h('p', { class: 'c-muted' }, `Could not load the settings: ${error.message}`));
    }
  }

  function field(label, control, help = null) {
    return h('div', { class: 'c-field' },
      h('label', { class: 'c-field__label', for: control.id }, label),
      control,
      help ? h('p', { class: 'c-field__hint' }, help) : null,
    );
  }

  function input(name, value, attributes = {}) {
    const isCheckbox = attributes.type === 'checkbox';
    const element = h('input', { class: isCheckbox ? null : 'c-input', id: `settings-${name}`, name, ...attributes, disabled: !data.can_edit });
    if (attributes.type === 'checkbox') element.checked = value;
    else element.value = value;
    return element;
  }

  function runtimeSelect() {
    const current = data.java.runtime.kind === 'runtime' ? data.java.runtime.id : 'auto';
    const options = [h('option', { value: 'auto' }, `Automatic (Java ${data.java.major}, installed when needed)`)];
    for (const runtime of data.runtimes) {
      const tooOld = runtime.major < data.java.major;
      const custom = !runtime.managed;
      const note = tooOld ? ' (too old)' : custom ? ' (not installed by the app)' : '';
      options.push(h('option', {
        value: runtime.id,
        disabled: tooOld || (custom && !data.can_use_custom_runtime),
      }, `Java ${runtime.version}${runtime.vendor ? ` · ${runtime.vendor}` : ''}${note}`));
    }
    const select = h('select', { class: 'c-input', id: 'settings-java_runtime', name: 'java_runtime', disabled: !data.can_edit }, options);
    select.value = current;
    return select;
  }

  function render() {
    const java = data.java;
    const behavior = data.behavior;
    form.replaceChildren(
      data.can_edit ? null : h('p', { class: 'c-banner is-info' }, 'You can view these settings but not change them.'),
      h('fieldset', { class: 'c-fieldset c-settings__group' }, h('legend', null, 'General'),
        field('Name', input('name', data.name, { maxlength: 64, required: true })),
      ),
      h('fieldset', { class: 'c-fieldset c-settings__group' }, h('legend', null, 'Memory'),
        field('Maximum memory (MB)', input('max_memory_mb', java.max_memory_mb, { type: 'number', min: 512, max: 65536, step: 256 }),
          'The most the server may use. Leave room for the system and other servers.'),
        field('Starting memory (MB)', input('min_memory_mb', java.min_memory_mb, { type: 'number', min: 512, max: 65536, step: 256 })),
      ),
      h('fieldset', { class: 'c-fieldset c-settings__group' }, h('legend', null, 'Java'),
        field('Java runtime', runtimeSelect()),
        field('Tuning preset', (() => {
          const select = h('select', { class: 'c-input', id: 'settings-jvm_preset', name: 'jvm_preset', disabled: !data.can_edit },
            h('option', { value: 'aikar' }, 'Aikar’s flags (recommended)'),
            h('option', { value: 'none' }, 'None (memory only)'));
          select.value = java.preset;
          return select;
        })()),
        field('Extra JVM arguments', (() => {
          const area = h('textarea', { class: 'c-input c-input--mono c-settings__args', id: 'settings-extra_jvm_args', name: 'extra_jvm_args', rows: 4, spellcheck: 'false', disabled: !data.can_edit });
          area.value = java.extra_jvm_args.join('\n');
          return area;
        })(), 'One per line, for example -XX:+UseZGC. Memory is set above.'),
      ),
      h('fieldset', { class: 'c-fieldset c-settings__group' }, h('legend', null, 'Behaviour'),
        h('label', { class: 'c-check' }, input('auto_start', behavior.auto_start, { type: 'checkbox' }), ' Start when the app starts'),
        h('label', { class: 'c-check' }, input('auto_restart', behavior.auto_restart, { type: 'checkbox' }), ' Restart automatically after a crash'),
        field('Crash limit', input('crash_limit', behavior.crash_limit, { type: 'number', min: 1, max: 20 }),
          'After this many crashes within 10 minutes, automatic restarts pause.'),
        field('Stop timeout (seconds)', input('stop_timeout_secs', behavior.stop_timeout_secs, { type: 'number', min: 5, max: 900 }),
          'How long a graceful stop may take before the server is killed.'),
      ),
      h('fieldset', { class: 'c-fieldset c-settings__group' }, h('legend', null, 'Backups'),
        field('Backups to keep', input('keep_backups', data.backups.keep, { type: 'number', min: 1, max: 100 }),
          'Older manual and scheduled backups are deleted after each new one. Pinned backups are always kept.'),
      ),
      data.can_edit ? h('div', { class: 'c-form__actions' }, h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Save settings')) : null,
    );
  }

  /** The fields that differ from what was loaded. */
  function changes() {
    const value = (name) => form.elements[name];
    const number = (name) => Number(value(name).value);
    const runtime = value('java_runtime').value;
    const current = {
      name: value('name').value.trim(),
      max_memory_mb: number('max_memory_mb'),
      min_memory_mb: number('min_memory_mb'),
      java_runtime: runtime === 'auto' ? { kind: 'auto' } : { kind: 'runtime', id: runtime },
      jvm_preset: value('jvm_preset').value,
      extra_jvm_args: value('extra_jvm_args').value.split('\n').map((line) => line.trim()).filter(Boolean),
      auto_start: value('auto_start').checked,
      auto_restart: value('auto_restart').checked,
      crash_limit: number('crash_limit'),
      stop_timeout_secs: number('stop_timeout_secs'),
      keep_backups: number('keep_backups'),
    };
    const loaded = {
      name: data.name,
      max_memory_mb: data.java.max_memory_mb,
      min_memory_mb: data.java.min_memory_mb,
      java_runtime: data.java.runtime,
      jvm_preset: data.java.preset,
      extra_jvm_args: data.java.extra_jvm_args,
      auto_start: data.behavior.auto_start,
      auto_restart: data.behavior.auto_restart,
      crash_limit: data.behavior.crash_limit,
      stop_timeout_secs: data.behavior.stop_timeout_secs,
      keep_backups: data.backups.keep,
    };
    return Object.fromEntries(Object.entries(current).filter(([key, next]) => JSON.stringify(next) !== JSON.stringify(loaded[key])));
  }

  async function save(event) {
    event.preventDefault();
    const change = changes();
    if (Object.keys(change).length === 0) {
      toast('Nothing to save.', { kind: 'info' });
      return;
    }
    const button = form.querySelector('[type="submit"]');
    button.disabled = true;
    try {
      data = await api.patch(`/instances/${instanceId}/settings`, change);
      render();
      onSaved?.();
      if (data.restart_required) {
        toast('Saved. Restart the server to apply the changes.', { kind: 'success', action: onRestart ? { label: 'Restart now', onClick: onRestart } : null });
      } else {
        toast('Settings saved.', { kind: 'success' });
      }
    } catch (error) {
      toast(error.message, { kind: 'error' });
      button.disabled = false;
    }
  }

  return { element, load };
}
