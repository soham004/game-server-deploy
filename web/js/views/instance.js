/**
 * One server: header with state and lifecycle buttons, then tabs (Console, Overview).
 * Live updates come from the "instances" and "jobs" topics.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { memoryLabel, shortDate, softwareLabel, stateKey, stateLabel } from '../core/format.js';
import { navigate } from '../core/navigation.js';
import { session } from '../core/session.js';
import { toast } from '../core/toast.js';
import { live } from '../core/ws.js';
import { createConsole } from '../components/console.js';

export async function mount(root, { id }) {
  let instance;
  try {
    instance = await api.get(`/instances/${id}`);
  } catch (error) {
    root.append(h('section', { class: 'c-card c-empty' },
      h('h1', { class: 'c-empty__title' }, 'Server not found'),
      h('p', { class: 'c-empty__text' }, error.message),
      h('a', { class: 'c-button', href: '/' }, 'Back to servers'),
    ));
    return () => {};
  }

  const can = (permission) => instance.permissions.includes(permission);
  let players = [];
  let diagnostics = [];

  const title = h('h1', { class: 'l-page-header__title' });
  const software = h('span', { class: 'c-badge' });
  const pill = h('span', { class: 'c-pill' });
  const actions = h('div', { class: 'c-actions' });
  const banner = h('div', { class: 'c-banner', role: 'status', hidden: true });
  const overview = h('div', { class: 'c-overview' });

  const terminal = can('console.read') ? createConsole({ instanceId: id, canWrite: can('console.write') }) : null;
  const tabs = createTabs([
    { id: 'console', label: 'Console', panel: terminal ? terminal.element : h('p', null, 'You cannot view this console.') },
    { id: 'overview', label: 'Overview', panel: overview },
  ]);

  root.append(
    h('header', { class: 'l-page-header' },
      h('div', { class: 'c-instance-heading' }, title, h('div', { class: 'c-instance-heading__meta' }, software, pill)),
      actions,
    ),
    banner,
    tabs.element,
  );

  function render() {
    title.textContent = instance.name;
    software.textContent = softwareLabel(instance.server);
    pill.textContent = stateLabel(instance);
    pill.className = `c-pill is-${stateKey(instance)}`;
    renderActions();
    renderOverview();
    terminal?.setRunning(['starting', 'running'].includes(instance.state.status) && !instance.busy);
  }

  function renderActions() {
    const status = instance.state.status;
    const active = ['starting', 'running', 'stopping'].includes(status);
    const buttons = [];
    if (can('lifecycle') && instance.installed && !instance.busy) {
      if (!active) buttons.push(actionButton('Start', 'start', 'c-button--primary'));
      if (status === 'running' || status === 'starting') {
        buttons.push(actionButton('Restart', 'restart'));
        buttons.push(actionButton('Stop', 'stop'));
      }
      if (active) buttons.push(actionButton('Kill', 'kill', 'c-button--danger', 'Force-stop the process. Unsaved world changes are lost.'));
    }
    if (can('instance.update') && !instance.installed && !instance.busy) {
      buttons.push(actionButton('Retry install', 'install', 'c-button--primary'));
    }
    if (session.canGlobal('instances.delete') && !active && !instance.busy) {
      buttons.push(h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: confirmDelete }, 'Delete'));
    }
    actions.replaceChildren(...buttons);
  }

  function actionButton(label, action, modifier = '', confirmText = null) {
    return h('button', {
      class: `c-button ${modifier}`.trim(),
      type: 'button',
      onClick: async (event) => {
        if (confirmText && !window.confirm(confirmText)) return;
        event.currentTarget.disabled = true;
        try {
          await api.post(`/instances/${id}/${action}`);
        } catch (error) {
          toast(error.message, { kind: 'error' });
        } finally {
          await refresh();
        }
      },
    }, label);
  }

  async function confirmDelete() {
    const typed = window.prompt(`Type "${instance.name}" to delete this server. Its folder is moved to the trash.`);
    if (typed === null) return;
    if (typed.trim() !== instance.name) {
      toast('The name did not match; nothing was deleted.', { kind: 'error' });
      return;
    }
    try {
      await api.delete(`/instances/${id}`);
      toast(`${instance.name} was deleted.`, { kind: 'success' });
      navigate('/');
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  function renderOverview() {
    const address = `${window.location.hostname}:${instance.port}`;
    overview.replaceChildren(
      h('dl', { class: 'c-details' },
        detail('Connect address', h('code', null, address)),
        detail('Software', softwareLabel(instance.server) + (instance.server.loader_version ? ` (${instance.server.loader_version})` : '')),
        detail('Java', `Java ${instance.java_major}`),
        detail('Memory', memoryLabel(instance.max_memory_mb)),
        detail('Created', shortDate(instance.created_at)),
        detail('Folder', h('code', null, instance.slug)),
      ),
      h('section', { class: 'c-overview__section' },
        h('h2', { class: 'c-overview__heading' }, `Players online (${players.length})`),
        players.length === 0
          ? h('p', { class: 'c-muted' }, 'Nobody is online.')
          : h('ul', { class: 'c-tag-list' }, players.map((name) => h('li', { class: 'c-tag' }, name))),
      ),
      diagnostics.length === 0 ? null : h('section', { class: 'c-overview__section' },
        h('h2', { class: 'c-overview__heading' }, 'Problems detected'),
        h('ul', { class: 'c-diagnostics' }, diagnostics.map((d) => h('li', { class: `c-diagnostic is-${d.severity}` },
          h('strong', null, d.title), h('p', null, d.detail),
        ))),
      ),
    );
  }

  function showJob(job) {
    if (job.status === 'running') {
      banner.hidden = false;
      const percent = job.progress === null ? '' : ` ${Math.round(job.progress * 100)}%`;
      banner.className = 'c-banner is-info';
      banner.textContent = `${job.step ?? job.title}…${percent}`;
    } else if (job.status === 'failed') {
      banner.hidden = false;
      banner.className = 'c-banner is-error';
      banner.textContent = `${job.title} failed: ${job.error ?? 'unknown error'}`;
    } else {
      banner.hidden = true;
    }
  }

  async function refresh() {
    try {
      instance = await api.get(`/instances/${id}`);
      if (terminal) {
        const snapshot = await api.get(`/instances/${id}/console`);
        players = snapshot.players;
        diagnostics = snapshot.diagnostics;
      }
      render();
    } catch (error) {
      if (error.status === 404) navigate('/');
    }
  }

  const offInstances = live.on('instances', (event) => {
    if (event.instance_id !== id) return;
    if (event.type === 'instance_deleted') {
      toast('This server was deleted.', { kind: 'info' });
      navigate('/');
    } else if (event.type === 'state') {
      instance.state = event.data;
      render();
    } else if (event.type === 'diagnostic') {
      diagnostics.push(event.data);
      renderOverview();
      toast(event.data.title, { kind: 'error' });
    } else {
      refresh();
    }
  });
  const offJobs = live.on('jobs', (event) => {
    if (event.type !== 'job' || event.data.instance_id !== id) return;
    showJob(event.data);
    if (event.data.status !== 'running') refresh();
  });
  const offResync = live.onResync(refresh);

  render();
  await refresh();

  return () => {
    offInstances();
    offJobs();
    offResync();
    terminal?.destroy();
  };
}

function detail(term, description) {
  return h('div', { class: 'c-details__row' }, h('dt', null, term), h('dd', null, description));
}

function createTabs(tabs) {
  const tablist = h('div', { class: 'c-tabs__list', role: 'tablist' });
  const panels = h('div', { class: 'c-tabs__panels' });
  const select = (selectedId) => {
    for (const button of tablist.children) {
      const active = button.dataset.tab === selectedId;
      button.setAttribute('aria-selected', String(active));
      button.tabIndex = active ? 0 : -1;
      button.classList.toggle('is-active', active);
    }
    for (const panel of panels.children) panel.hidden = panel.dataset.tab !== selectedId;
  };
  for (const tab of tabs) {
    tablist.append(h('button', {
      class: 'c-tabs__tab', type: 'button', role: 'tab', dataset: { tab: tab.id }, onClick: () => select(tab.id),
    }, tab.label));
    panels.append(h('div', { class: 'c-tabs__panel', role: 'tabpanel', dataset: { tab: tab.id } }, tab.panel));
  }
  select(tabs[0].id);
  return { element: h('div', { class: 'c-tabs' }, tablist, panels) };
}
