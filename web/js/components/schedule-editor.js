/**
 * The form for one schedule: name, when it runs, conditions, the list of actions, options.
 *
 *   const editor = createScheduleEditor({ plan, onSave: async (plan) => …, onCancel });
 *   panel.append(editor.element);
 *
 * It edits a copy of `plan` (the server's `SchedulePlan` JSON) and re-renders when the shape
 * changes (another trigger kind, an action added or moved). The server checks everything again.
 */
import { h } from '../core/dom.js';
import {
  browserTimeZone, dailyCron, dailyTime, formatWarnings, parseWarnings, splitDuration,
} from '../core/schedule-plan.js';
import { toast } from '../core/toast.js';

const WHEN = [
  ['daily', 'Every day at'],
  ['interval', 'Every'],
  ['once', 'Once, at'],
  ['event', 'When something happens'],
  ['cron', 'Cron expression'],
];

const EVENTS = [
  ['started', 'The server has started'],
  ['stopped', 'The server stops'],
  ['crashed', 'The server crashes'],
  ['player_joined', 'A player joins'],
  ['empty_for', 'Nobody has been online for…'],
];

const ACTIONS = [
  ['command', 'Run a console command'],
  ['broadcast', 'Show a message to players'],
  ['restart', 'Restart the server'],
  ['stop', 'Stop the server'],
  ['start', 'Start the server'],
  ['backup', 'Back up'],
  ['wait', 'Wait'],
];

const UNITS = [[60, 'minutes'], [3600, 'hours'], [86400, 'days']];

/** A fresh action of `kind` with sensible defaults. */
function newAction(kind) {
  switch (kind) {
    case 'command': return { kind, command: '' };
    case 'broadcast': return { kind, message: '' };
    case 'restart': case 'stop': return { kind, warn_secs: [300, 60, 10] };
    case 'backup': return { kind, scope: 'world' };
    case 'wait': return { kind, secs: 60 };
    default: return { kind };
  }
}

export function createScheduleEditor({ plan, onSave, onCancel }) {
  const draft = structuredClone(plan);
  let when = whenOf(draft.trigger);
  const form = h('form', { class: 'c-form c-schedule-editor', novalidate: true, onSubmit: save });

  function whenOf(trigger) {
    if (trigger.kind === 'cron') return dailyTime(trigger.expr) ? 'daily' : 'cron';
    return trigger.kind;
  }

  function field(label, control, hint = null) {
    return h('label', { class: 'c-field' }, h('span', { class: 'c-field__label' }, label), control, hint ? h('span', { class: 'c-field__hint' }, hint) : null);
  }

  function input(value, onInput, attributes = {}) {
    const element = h('input', { class: 'c-input', ...attributes });
    element.value = value ?? '';
    element.addEventListener('input', () => onInput(element.value));
    return element;
  }

  function select(options, value, onChange, attributes = {}) {
    const element = h('select', { class: 'c-input', ...attributes }, options.map(([key, label]) => h('option', { value: key }, label)));
    element.value = String(value);
    element.addEventListener('change', () => onChange(element.value));
    return element;
  }

  function checkbox(label, checked, onChange) {
    const box = h('input', { type: 'checkbox' });
    box.checked = checked;
    box.addEventListener('change', () => onChange(box.checked));
    return h('label', { class: 'c-check' }, box, ` ${label}`);
  }

  function render() {
    form.replaceChildren(
      field('Name', input(draft.name, (v) => { draft.name = v; }, { maxlength: 64, required: true, 'aria-label': 'Schedule name', name: 'name' })),
      h('fieldset', { class: 'c-fieldset' }, h('legend', null, 'When'), triggerFields()),
      h('fieldset', { class: 'c-fieldset' }, h('legend', null, 'Only if'), conditionFields()),
      h('fieldset', { class: 'c-fieldset' }, h('legend', null, 'Then'), actionFields()),
      h('fieldset', { class: 'c-fieldset' }, h('legend', null, 'Options'),
        checkbox('Enabled', draft.enabled, (v) => { draft.enabled = v; }),
        checkbox('If it was due while the app was off, run it once when the app starts', draft.catch_up, (v) => { draft.catch_up = v; }),
        field('If an action fails', select([['stop', 'Skip the remaining actions'], ['continue', 'Go on with the next action']], draft.on_error, (v) => { draft.on_error = v; })),
      ),
      h('div', { class: 'c-form__actions' },
        h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Save schedule'),
        h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: onCancel }, 'Cancel'),
      ),
    );
  }

  function triggerFields() {
    const trigger = draft.trigger;
    const kind = select(WHEN, when, (value) => { when = value; draft.trigger = defaultTrigger(value); render(); }, { 'aria-label': 'When', name: 'when' });
    const zone = trigger.time_zone ?? browserTimeZone();
    let detail;
    if (when === 'daily') {
      detail = field('Time', input(dailyTime(trigger.expr), (v) => {
        try { trigger.expr = dailyCron(v); } catch { /* checked on save */ }
      }, { type: 'time', name: 'daily_time' }), `In ${zone}`);
    } else if (when === 'cron') {
      detail = [
        field('Cron expression', input(trigger.expr, (v) => { trigger.expr = v; }, { class: 'c-input c-input--mono', name: 'cron' }),
          'Minute hour day month weekday, e.g. 0 */6 * * * (every 6 hours). Add a first field for seconds.'),
        field('Time zone', input(trigger.time_zone, (v) => { trigger.time_zone = v.trim(); }, { name: 'time_zone' })),
      ];
    } else if (when === 'interval') {
      const { count, unit } = splitDuration(trigger.every_secs);
      const current = { count, unit };
      const update = () => { trigger.every_secs = Math.round(current.count * current.unit); };
      detail = h('div', { class: 'c-schedule-editor__row' },
        input(count, (v) => { current.count = Number(v); update(); }, { type: 'number', min: 1, 'aria-label': 'Every how many', name: 'every' }),
        select(UNITS, unit, (v) => { current.unit = Number(v); update(); }, { 'aria-label': 'Unit' }));
    } else if (when === 'once') {
      detail = field('Date and time', input(localInput(trigger.at), (v) => { trigger.at = v ? new Date(v).toISOString() : ''; }, { type: 'datetime-local', name: 'once_at' }));
    } else {
      const event = trigger.event;
      detail = [
        select(EVENTS, event.type, (value) => { draft.trigger = { kind: 'event', event: value === 'empty_for' ? { type: value, secs: 900 } : { type: value } }; render(); }, { 'aria-label': 'Event', name: 'event' }),
        event.type === 'empty_for'
          ? field('Minutes', input(event.secs / 60, (v) => { event.secs = Math.round(Number(v) * 60); }, { type: 'number', min: 1, max: 1440 }))
          : null,
      ];
    }
    return [kind, detail];
  }

  function defaultTrigger(kind) {
    const zone = browserTimeZone();
    switch (kind) {
      case 'daily': return { kind: 'cron', expr: '0 4 * * *', time_zone: zone };
      case 'cron': return { kind: 'cron', expr: '0 */6 * * *', time_zone: zone };
      case 'interval': return { kind: 'interval', every_secs: 3600 };
      case 'once': return { kind: 'once', at: new Date(Date.now() + 3600_000).toISOString() };
      default: return { kind: 'event', event: { type: 'started' } };
    }
  }

  function conditionFields() {
    const has = (kind) => draft.conditions.find((c) => c.kind === kind);
    const toggle = (kind, make) => (on) => {
      draft.conditions = draft.conditions.filter((c) => c.kind !== kind);
      if (on) draft.conditions.push(make());
      render();
    };
    const players = has('players_online');
    return [
      checkbox('The server is running', Boolean(has('server_running')), toggle('server_running', () => ({ kind: 'server_running' }))),
      checkbox('The server is stopped', Boolean(has('server_stopped')), toggle('server_stopped', () => ({ kind: 'server_stopped' }))),
      checkbox('Somebody played since the last run', Boolean(has('players_since_last_run')), toggle('players_since_last_run', () => ({ kind: 'players_since_last_run' }))),
      h('div', { class: 'c-schedule-editor__row' },
        checkbox('At least this many players are online:', Boolean(players), toggle('players_online', () => ({ kind: 'players_online', min: 1 }))),
        players ? input(players.min, (v) => { players.min = Number(v); }, { type: 'number', min: 1, 'aria-label': 'Minimum players' }) : null),
      timeWindowFields(has('time_window'), toggle),
    ];
  }

  /** "Between 22:00 and 06:00" (the server stores seconds too: "22:00:00"). */
  function timeWindowFields(window, toggle) {
    const zone = browserTimeZone();
    const hhmm = (time) => time.slice(0, 5);
    const row = h('div', { class: 'c-schedule-editor__row' },
      checkbox('It is between', Boolean(window), toggle('time_window', () => ({ kind: 'time_window', from: '22:00:00', to: '06:00:00', time_zone: zone }))));
    if (window) {
      row.append(
        input(hhmm(window.from), (v) => { window.from = `${v}:00`; }, { type: 'time', 'aria-label': 'From', name: 'window_from' }),
        h('span', null, 'and'),
        input(hhmm(window.to), (v) => { window.to = `${v}:00`; }, { type: 'time', 'aria-label': 'To', name: 'window_to' }),
        h('span', { class: 'c-muted' }, `(${window.time_zone})`),
      );
    }
    return row;
  }

  function actionFields() {
    const rows = draft.actions.map((action, index) => h('div', { class: 'c-schedule-editor__action' },
      h('span', { class: 'c-schedule-editor__step' }, `${index + 1}.`),
      select(ACTIONS, action.kind, (kind) => { draft.actions[index] = newAction(kind); render(); }, { 'aria-label': `Action ${index + 1}` }),
      actionDetail(action, index),
      h('span', { class: 'c-schedule-editor__tools' },
        h('button', { class: 'c-button c-button--ghost', type: 'button', title: 'Move up', 'aria-label': 'Move up', disabled: index === 0, onClick: () => move(index, -1) }, '↑'),
        h('button', { class: 'c-button c-button--ghost', type: 'button', title: 'Move down', 'aria-label': 'Move down', disabled: index === draft.actions.length - 1, onClick: () => move(index, 1) }, '↓'),
        h('button', { class: 'c-button c-button--danger-ghost', type: 'button', title: 'Remove', 'aria-label': 'Remove', onClick: () => { draft.actions.splice(index, 1); render(); } }, '✕'),
      ),
    ));
    const add = h('button', { class: 'c-button', type: 'button', onClick: () => { draft.actions.push(newAction('command')); render(); } }, 'Add an action');
    return [...rows, add];
  }

  function actionDetail(action, index) {
    switch (action.kind) {
      case 'command':
        return input(action.command, (v) => { action.command = v; }, { class: 'c-input c-input--mono', placeholder: 'save-all', 'aria-label': `Command ${index + 1}`, name: `command-${index}` });
      case 'broadcast':
        return input(action.message, (v) => { action.message = v; }, { placeholder: 'Message', 'aria-label': `Message ${index + 1}`, name: `message-${index}` });
      case 'restart': case 'stop':
        return input(formatWarnings(action.warn_secs), (v) => { action.warnText = v; }, {
          placeholder: 'Warnings, e.g. 10m, 5m, 1m, 10s', title: 'When to warn the players before, latest last', 'aria-label': `Warnings ${index + 1}`, name: `warnings-${index}`,
        });
      case 'backup':
        return select([['world', 'Worlds only'], ['full', 'Everything']], action.scope, (v) => { action.scope = v; }, { 'aria-label': `Backup scope ${index + 1}` });
      case 'wait':
        return input(action.secs, (v) => { action.secs = Number(v); }, { type: 'number', min: 1, max: 3600, 'aria-label': `Seconds ${index + 1}`, title: 'Seconds' });
      default:
        return null;
    }
  }

  function move(index, step) {
    const [action] = draft.actions.splice(index, 1);
    draft.actions.splice(index + step, 0, action);
    render();
  }

  /** The plan to send: warning text parsed, editor-only fields removed. */
  function finished() {
    const plan = structuredClone(draft);
    plan.name = plan.name.trim();
    plan.actions = plan.actions.map((action) => {
      if (action.warnText === undefined) return action;
      const { warnText, ...rest } = action;
      return { ...rest, warn_secs: parseWarnings(warnText) };
    });
    return plan;
  }

  async function save(event) {
    event.preventDefault();
    let plan;
    try {
      plan = finished();
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return;
    }
    const button = form.querySelector('[type="submit"]');
    button.disabled = true;
    try {
      await onSave(plan);
    } finally {
      button.disabled = false;
    }
  }

  render();
  return { element: form };
}

/** An ISO time as a `datetime-local` value in the browser's time zone. */
function localInput(iso) {
  if (!iso) return '';
  const date = new Date(iso);
  const pad = (n) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
