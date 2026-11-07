/**
 * Users (owners only): add accounts, change roles, disable, reset passwords, sign people out,
 * delete, and give members access to single servers.
 *
 * Temporary passwords are shown once, right after they are made.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';
import { relativeTime } from '../core/format.js';
import { session } from '../core/session.js';
import { toast } from '../core/toast.js';

const ROLES = [
  ['owner', 'Owner', 'Everything, including users'],
  ['admin', 'Admin', 'Every server; no user management'],
  ['member', 'Member', 'Only the servers they are given access to'],
];
const GRANTS = [
  ['viewer', 'Viewer: see the server and its console'],
  ['moderator', 'Moderator: also manage players'],
  ['operator', 'Operator: also start, stop and send commands'],
  ['manager', 'Manager: everything on this server'],
];

export async function mount(root) {
  if (!session.canGlobal('users.manage')) {
    root.append(h('section', { class: 'c-card c-empty' }, h('h1', { class: 'c-empty__title' }, 'Only owners can manage users.')));
    return () => {};
  }
  const secret = h('div', { class: 'c-banner is-info c-users__secret', role: 'status', hidden: true });
  const table = h('table', { class: 'c-users__table' });
  const access = h('section', { class: 'c-card c-users__access', hidden: true });
  const addForm = h('form', { class: 'c-form c-users__add', novalidate: true },
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'new-username' }, 'Username'),
      h('input', { class: 'c-input', id: 'new-username', name: 'username', autocomplete: 'off', required: true })),
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'new-role' }, 'Role'),
      h('select', { class: 'c-input', id: 'new-role', name: 'role' }, ROLES.map(([value, label]) => h('option', { value, selected: value === 'member' }, label)))),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' }, h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Add user')),
  );
  root.append(
    h('header', { class: 'l-page-header' }, h('h1', { class: 'l-page-header__title' }, 'Users')),
    h('div', { class: 'c-users' },
      secret,
      h('section', { class: 'c-card' }, table),
      h('section', { class: 'c-card' }, h('h2', { class: 'c-users__heading' }, 'Add a user'), addForm),
      access),
  );

  let users = [];
  let instances = [];

  onSubmit(addForm, async (values) => {
    const created = await api.post('/users', { username: values.username, role: values.role });
    addForm.reset();
    showSecret(created.user.username, created.temporary_password);
    await refresh();
  });

  async function refresh() {
    try {
      [users, instances] = await Promise.all([api.get('/users'), api.get('/instances')]);
      render();
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
  }

  function render() {
    const me = session.me?.user?.id;
    table.replaceChildren(
      h('thead', null, h('tr', null, h('th', null, 'User'), h('th', null, 'Role'), h('th', null, 'Status'), h('th', null, 'Signed in'), h('th', null, ''))),
      h('tbody', null, users.map((user) => row(user, user.id === me))),
    );
  }

  function row(user, isMe) {
    const role = h('select', { class: 'c-input', 'aria-label': `Role of ${user.username}`, disabled: isMe,
      onChange: (event) => change(user, { role: event.target.value }) },
    ROLES.map(([value, label, hint]) => h('option', { value, title: hint }, label)));
    role.value = user.role;
    const status = user.disabled ? 'Disabled' : user.must_change_password ? 'Must set a password' : 'Active';
    const actions = isMe ? [h('span', { class: 'c-muted' }, 'You')] : [
      user.role === 'member' ? button('Access', () => showAccess(user)) : null,
      button(user.disabled ? 'Enable' : 'Disable', () => change(user, { disabled: !user.disabled })),
      button('Reset password', () => resetPassword(user)),
      user.active_sessions > 0 ? button('Sign out', () => signOut(user)) : null,
      button('Delete', () => remove(user), 'c-button--danger-ghost'),
    ];
    return h('tr', { class: user.disabled ? 'is-disabled' : '', dataset: { user: user.username } },
      h('td', null, h('strong', null, user.username), h('div', { class: 'c-muted' }, `added ${relativeTime(user.created_at)}`)),
      h('td', null, role),
      h('td', null, status),
      h('td', null, user.active_sessions === 0 ? 'No' : `${user.active_sessions} session${user.active_sessions === 1 ? '' : 's'}`),
      h('td', { class: 'c-users__actions' }, actions),
    );
  }

  function button(label, onClick, modifier = '') {
    return h('button', { class: `c-button c-button--ghost ${modifier}`.trim(), type: 'button', onClick }, label);
  }

  async function change(user, body) {
    try {
      await api.patch(`/users/${user.id}`, body);
      toast(`${user.username} updated.`, { kind: 'success' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  async function resetPassword(user) {
    if (!window.confirm(`Give ${user.username} a new temporary password? They are signed out everywhere.`)) return;
    try {
      const { temporary_password: password } = await api.post(`/users/${user.id}/reset-password`);
      showSecret(user.username, password);
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  async function signOut(user) {
    try {
      await api.post(`/users/${user.id}/sign-out`);
      toast(`${user.username} was signed out everywhere.`, { kind: 'success' });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  async function remove(user) {
    if (!window.confirm(`Delete ${user.username}? This cannot be undone.`)) return;
    try {
      await api.delete(`/users/${user.id}`);
      toast(`${user.username} was deleted.`, { kind: 'success' });
      access.hidden = true;
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await refresh();
  }

  function showSecret(username, password) {
    const code = h('code', { class: 'c-users__password' }, password);
    secret.replaceChildren(
      h('p', null, 'Temporary password for ', h('strong', null, username), ' (shown only now; they must change it when they sign in):'),
      h('div', { class: 'c-users__secret-row' }, code,
        button('Copy', async () => {
          try {
            await navigator.clipboard.writeText(password);
            toast('Copied.', { kind: 'success' });
          } catch {
            toast('Copy it by hand: the clipboard is not available.', { kind: 'info' });
          }
        }),
        button('Hide', () => { secret.hidden = true; })),
    );
    secret.hidden = false;
  }

  // ---- per-server access for members ----

  async function showAccess(user) {
    let grants = [];
    try {
      grants = await api.get(`/users/${user.id}/grants`);
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return;
    }
    const granted = new Set(grants.map((g) => g.instance_id));
    const grantSelect = (value, onChange) => {
      const select = h('select', { class: 'c-input', onChange }, GRANTS.map(([v, label]) => h('option', { value: v }, label)));
      if (value === 'custom') select.append(h('option', { value: 'custom' }, 'Custom'));
      select.value = value;
      return select;
    };
    const rows = grants.map((grant) => h('li', { class: 'c-users__grant' },
      h('span', null, grant.instance_name ?? '(deleted server)'),
      grantSelect(grant.grant, (event) => setGrant(user, grant.instance_id, event.target.value)),
      button('Remove', () => removeGrant(user, grant.instance_id), 'c-button--danger-ghost'),
    ));
    const available = instances.filter((instance) => !granted.has(instance.id));
    const newServer = h('select', { class: 'c-input', 'aria-label': 'Server' }, available.map((i) => h('option', { value: i.id }, i.name)));
    const newGrant = grantSelect('viewer', () => {});
    access.replaceChildren(
      h('h2', { class: 'c-users__heading' }, `Servers ${user.username} can use`),
      rows.length > 0 ? h('ul', { class: 'c-users__grants' }, rows) : h('p', { class: 'c-muted' }, 'None yet.'),
      available.length > 0 ? h('div', { class: 'c-users__grant-add' }, newServer, newGrant,
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => setGrant(user, newServer.value, newGrant.value) }, 'Give access'))
        : null,
    );
    access.hidden = false;
    access.scrollIntoView({ block: 'nearest' });
  }

  async function setGrant(user, instanceId, grant) {
    try {
      await api.put(`/users/${user.id}/grants/${instanceId}`, { grant });
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await showAccess(user);
  }

  async function removeGrant(user, instanceId) {
    try {
      await api.delete(`/users/${user.id}/grants/${instanceId}`);
    } catch (error) {
      toast(error.message, { kind: 'error' });
    }
    await showAccess(user);
  }

  await refresh();
  return () => {};
}
