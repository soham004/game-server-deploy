/**
 * A server's schedules: what each does and when it runs next, its last result, run history,
 * and (with `schedules.manage`) new, edit, enable, run now and delete.
 *
 *   const schedules = createSchedules({ instanceId, can });
 *   panel.append(schedules.element);
 *   await schedules.load();
 *
 * The list reloads when the server reports a change (`schedules_changed`, also sent after
 * every run), so every open page stays current.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { dateTime, untilTime } from '../core/format.js';
import { actionsSummary, blankPlan, browserTimeZone, templates } from '../core/schedule-plan.js';
import { toast } from '../core/toast.js';
import { live } from '../core/ws.js';
import { createScheduleEditor } from './schedule-editor.js';

const OUTCOME_LABELS = {
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
  skipped: 'Skipped',
};

const CAUSE_LABELS = {
  trigger: 'On time',
  catch_up: 'Caught up',
  manual: 'Run now',
};

export function createSchedules({ instanceId, can }) {
  const url = `/instances/${instanceId}/schedules`;
  const canManage = can('schedules.manage');
  let schedules = [];
  let editing = null; // null, or { id: string | null, plan }
  const openHistory = new Set();
  const toolbar = h('div', { class: 'c-schedules__toolbar' });
  const body = h('div', { class: 'c-schedules__body' });
  const element = h('section', { class: 'c-schedules' }, toolbar, body);

  async function load() {
    try {
      schedules = (await api.get(url)).schedules;
      render();
    } catch (error) {
      body.replaceChildren(h('p', { class: 'c-muted' }, `Could not load the schedules: ${error.message}`));
    }
  }

  function render() {
    renderToolbar();
    if (editing) {
      const editor = createScheduleEditor({ plan: editing.plan, onSave: save, onCancel: () => { editing = null; render(); } });
      body.replaceChildren(h('h2', { class: 'c-schedules__heading' }, editing.id ? 'Edit schedule' : 'New schedule'), editor.element);
      return;
    }
    if (schedules.length === 0) {
      body.replaceChildren(h('p', { class: 'c-muted' },
        canManage ? 'No schedules yet. Start from a template or create one.' : 'This server has no schedules.'));
      return;
    }
    body.replaceChildren(h('ul', { class: 'c-schedules__list' }, schedules.map(item)));
    for (const id of openHistory) fillHistory(id);
  }

  function renderToolbar() {
    if (!canManage || editing) {
      toolbar.replaceChildren();
      return;
    }
    const zone = browserTimeZone();
    const fromTemplate = h('select', { class: 'c-input', 'aria-label': 'Start from a template' },
      h('option', { value: '' }, 'Start from a template…'),
      templates(zone).map((template) => h('option', { value: template.id }, template.label)));
    fromTemplate.addEventListener('change', () => {
      const template = templates(zone).find((t) => t.id === fromTemplate.value);
      if (template) edit(null, template.plan);
    });
    toolbar.replaceChildren(
      h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => edit(null, blankPlan(zone)) }, 'New schedule'),
      fromTemplate,
    );
  }

  function item(schedule) {
    const { plan } = schedule;
    const last = schedule.last_run;
    return h('li', { class: `c-schedule${plan.enabled ? '' : ' is-disabled'}`, dataset: { schedule: schedule.id } },
      h('div', { class: 'c-schedule__main' },
        h('div', { class: 'c-schedule__title' },
          h('strong', null, plan.name),
          plan.enabled ? null : h('span', { class: 'c-badge' }, 'Disabled'),
          last ? h('span', { class: `c-pill is-${pillState(last.outcome)}`, title: last.message ?? '' }, OUTCOME_LABELS[last.outcome] ?? last.outcome) : null),
        h('p', { class: 'c-schedule__when' }, schedule.description, nextRun(schedule)),
        h('p', { class: 'c-muted c-schedule__what' }, actionsSummary(plan.actions)),
        last?.message ? h('p', { class: `c-schedule__message is-${last.outcome}` }, last.message) : null,
        schedule.run_as ? null : h('p', { class: 'c-schedule__message is-failed' }, 'Nobody to run it as: save it again to run it with your permissions.'),
      ),
      h('div', { class: 'c-schedule__actions' }, actions(schedule)),
      openHistory.has(schedule.id) ? h('div', { class: 'c-schedule__history', dataset: { history: schedule.id } }, 'Loading…') : null,
    );
  }

  function nextRun(schedule) {
    if (!schedule.plan.enabled || !schedule.next_run_at) return null;
    return h('span', { class: 'c-muted', title: dateTime(schedule.next_run_at) }, ` · next ${untilTime(schedule.next_run_at)}`);
  }

  function actions(schedule) {
    const buttons = [button(openHistory.has(schedule.id) ? 'Hide history' : 'History', () => toggleHistory(schedule))];
    if (canManage) {
      buttons.unshift(
        button('Run now', () => runNow(schedule)),
        button('Edit', () => edit(schedule.id, schedule.plan)),
        button(schedule.plan.enabled ? 'Disable' : 'Enable', () => setEnabled(schedule, !schedule.plan.enabled)),
      );
      buttons.push(button('Delete', () => remove(schedule), 'c-button--danger-ghost'));
    }
    return buttons;
  }

  function button(label, onClick, modifier = 'c-button--ghost') {
    return h('button', { class: `c-button ${modifier}`, type: 'button', onClick }, label);
  }

  function edit(id, plan) {
    editing = { id, plan: structuredClone(plan) };
    render();
  }

  async function save(plan) {
    try {
      if (editing.id) {
        await api.put(`${url}/${editing.id}`, plan);
        toast('Schedule saved.', { kind: 'success' });
      } else {
        await api.post(url, plan);
        toast('Schedule created.', { kind: 'success' });
      }
      editing = null;
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function setEnabled(schedule, enabled) {
    try {
      await api.put(`${url}/${schedule.id}`, { ...schedule.plan, enabled });
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function runNow(schedule) {
    try {
      await api.post(`${url}/${schedule.id}/run`);
      toast(`Running "${schedule.plan.name}"…`, { kind: 'info' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  async function remove(schedule) {
    if (!window.confirm(`Delete the schedule "${schedule.plan.name}"?`)) return;
    try {
      await api.delete(`${url}/${schedule.id}`);
      await load();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  function toggleHistory(schedule) {
    if (openHistory.has(schedule.id)) openHistory.delete(schedule.id);
    else openHistory.add(schedule.id);
    render();
  }

  async function fillHistory(id) {
    const panel = body.querySelector(`[data-history="${id}"]`);
    if (!panel) return;
    try {
      const runs = await api.get(`${url}/${id}/runs`);
      panel.replaceChildren(runs.length === 0
        ? h('p', { class: 'c-muted' }, 'It has not run yet.')
        : h('ol', { class: 'c-schedule__runs' }, runs.map((run) => h('li', null,
          h('span', { class: `c-pill is-${pillState(run.outcome)}` }, OUTCOME_LABELS[run.outcome] ?? run.outcome),
          h('span', null, ` ${dateTime(run.started_at)} · ${CAUSE_LABELS[run.cause] ?? run.cause}`),
          run.message ? h('span', { class: 'c-muted' }, ` · ${run.message}`) : null,
        ))));
    } catch (error) {
      panel.textContent = `Could not load the history: ${error.message}`;
    }
  }

  const offInstances = live.on('instances', (event) => {
    if (event.instance_id === instanceId && event.type === 'schedules_changed' && !editing) load();
  });

  return { element, load, destroy: offInstances };
}

/** The pill colour of a run outcome: a run in progress looks busy, not "running" green. */
function pillState(outcome) {
  return outcome === 'running' ? 'busy' : outcome;
}
