/**
 * Dashboard: the list of servers. Instances arrive in milestone M1; for now it shows the
 * empty state appropriate to the user's role.
 */
import { h } from '../core/dom.js';
import { session } from '../core/session.js';

export function mount(root) {
  const canCreate = session.canGlobal('instances.create');

  root.append(
    h('header', { class: 'l-page-header' },
      h('h1', { class: 'l-page-header__title' }, 'Servers'),
      canCreate
        ? h('button', { class: 'c-button c-button--primary', type: 'button', disabled: true, title: 'Coming soon' }, 'New server')
        : null,
    ),
    h('section', { class: 'c-card c-empty' },
      h('h2', { class: 'c-empty__title' }, 'No servers yet'),
      h('p', { class: 'c-empty__text' },
        canCreate
          ? 'Servers you create will appear here with their status, players and resource use.'
          : 'When an admin gives you access to a server, it will appear here.',
      ),
    ),
  );

  return () => {};
}
