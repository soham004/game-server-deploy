/**
 * App shell boot sequence. The server only hands this page to signed-in users (it sends anyone
 * else to the sign-in or setup page), but the session can still end before `/me` answers:
 *   1. not signed in            → /login.html?next=…
 *   2. must change password     → /login.html?change=1
 *   3. otherwise show the shell, connect the live stream and start the router.
 */
import { api, ApiError } from './core/api.js';
import { $ } from './core/dom.js';
import { setNavigator } from './core/navigation.js';
import { createRouter } from './core/router.js';
import { session } from './core/session.js';
import { live } from './core/ws.js';
import { mountJobTray } from './components/job-tray.js';

// Order matters: "/servers/new" must match before "/servers/:id".
const routes = [
  { path: '/', view: () => loadView(() => import('./views/dashboard.js')) },
  { path: '/servers/new', view: () => loadView(() => import('./views/instance-create.js')) },
  { path: '/servers/import', view: () => loadView(() => import('./views/instance-import.js')) },
  { path: '/servers/:id', view: () => loadView(() => import('./views/instance.js')) },
  { path: '/users', view: () => loadView(() => import('./views/users.js')) },
];

/**
 * Views load when first opened, and the server only hands out the app's files to signed-in
 * users. So a view that fails to load may mean the session ended: then go to the sign-in page
 * (and come back here afterwards) instead of showing an error.
 */
async function loadView(importView) {
  try {
    return await importView();
  } catch (error) {
    if (!(await loadCurrentUser())) return new Promise(() => {}); // leaving for the sign-in page
    throw error;
  }
}

async function boot() {
  const me = await loadCurrentUser();
  if (!me) return;
  session.setMe(me);
  showShell(me);
  live.start();
  mountJobTray($('[data-job-tray]'));

  const router = createRouter({
    root: $('#view'),
    routes,
    notFound: () => loadView(() => import('./views/not-found.js')),
    onNavigate: highlightNav,
  });
  setNavigator(router.navigate);
  await router.start();
}

/** Returns the current user, or null after redirecting to the sign-in page. */
async function loadCurrentUser() {
  try {
    const me = await api.get('/me');
    if (me.user.must_change_password) {
      window.location.replace('/login.html?change=1');
      return null;
    }
    return me;
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      const next = encodeURIComponent(window.location.pathname + window.location.search);
      window.location.replace(`/login.html?next=${next}`);
      return null;
    }
    throw error;
  }
}

function showShell(me) {
  $('[data-username]').textContent = me.user.username;
  // Links to pages only some roles may use.
  for (const link of document.querySelectorAll('[data-requires]')) link.hidden = !session.canGlobal(link.dataset.requires);
  $('[data-action="logout"]').addEventListener('click', logout);
  $('[data-boot]').hidden = true;
  $('[data-shell]').hidden = false;
}

async function logout() {
  try {
    await api.post('/auth/logout');
  } finally {
    window.location.assign('/login.html');
  }
}

function highlightNav(pathname) {
  for (const link of document.querySelectorAll('[data-nav]')) {
    const target = link.dataset.nav;
    const active = target === '/' ? pathname === '/' || pathname.startsWith('/servers') : pathname.startsWith(target);
    link.classList.toggle('is-active', active);
    if (active) link.setAttribute('aria-current', 'page');
    else link.removeAttribute('aria-current');
  }
}

boot().catch((error) => {
  console.error(error);
  $('[data-boot]').textContent = `Could not start: ${error.message}`;
});
