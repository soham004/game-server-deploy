/** Dashboard: one card per server the user can see, updated live. */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { memoryLabel, softwareLabel, stateKey, stateLabel } from '../core/format.js';
import { session } from '../core/session.js';
import { live } from '../core/ws.js';

export async function mount(root) {
  const canCreate = session.canGlobal('instances.create');
  const grid = h('div', { class: 'c-server-grid' });
  const hostProblem = h('div', { class: 'c-banner is-error', role: 'alert', hidden: true });
  let instances = [];

  root.append(
    h('header', { class: 'l-page-header' },
      h('h1', { class: 'l-page-header__title' }, 'Servers'),
      canCreate ? h('a', { class: 'c-button c-button--primary', href: '/servers/new' }, 'New server') : null,
    ),
    hostProblem,
    grid,
  );

  // A host-level problem (e.g. the sandbox can't work in the data folder) blocks every server.
  api.get('/system').then((system) => {
    if (!system.sandbox_problem) return;
    hostProblem.replaceChildren(
      h('strong', { class: 'c-banner__title' }, 'Servers can’t start on this host'),
      h('p', null, system.sandbox_problem),
    );
    hostProblem.hidden = false;
  }).catch(() => {});

  function render() {
    if (instances.length === 0) {
      grid.replaceChildren(h('section', { class: 'c-card c-empty' },
        h('h2', { class: 'c-empty__title' }, 'No servers yet'),
        h('p', { class: 'c-empty__text' },
          canCreate
            ? 'Create your first server: pick a Minecraft version and server software, accept the EULA, and it is ready in about a minute.'
            : 'When an admin gives you access to a server, it will appear here.'),
        canCreate ? h('a', { class: 'c-button c-button--primary', href: '/servers/new' }, 'New server') : null,
      ));
      return;
    }
    grid.replaceChildren(...instances.map(card));
  }

  async function refresh() {
    try {
      instances = await api.get('/instances');
    } catch {
      return;
    }
    render();
  }

  const offInstances = live.on('instances', (event) => {
    const instance = instances.find((i) => i.id === event.instance_id);
    if (event.type === 'state' && instance) {
      instance.state = event.data;
      render();
    } else if (event.type === 'instance_changed' || event.type === 'instance_deleted') {
      refresh();
    }
  });
  const offJobs = live.on('jobs', (event) => {
    if (event.type === 'job' && event.data.status !== 'running' && event.data.instance_id) refresh();
  });
  const offResync = live.onResync(refresh);

  await refresh();
  return () => {
    offInstances();
    offJobs();
    offResync();
  };
}

function card(instance) {
  return h('a', { class: `c-server-card is-${stateKey(instance)}`, href: `/servers/${instance.id}` },
    h('div', { class: 'c-server-card__top' },
      h('h2', { class: 'c-server-card__name' }, instance.name),
      h('span', { class: `c-pill is-${stateKey(instance)}` }, stateLabel(instance)),
    ),
    h('p', { class: 'c-server-card__software' }, softwareLabel(instance)),
    h('dl', { class: 'c-server-card__facts' },
      h('div', null, h('dt', null, 'Port'), h('dd', null, String(instance.port))),
      h('div', null, h('dt', null, 'Memory'), h('dd', null, memoryLabel(instance.max_memory_mb))),
      instance.modpack ? h('div', null, h('dt', null, 'Modpack'), h('dd', null, instance.modpack.name)) : null,
    ),
  );
}
