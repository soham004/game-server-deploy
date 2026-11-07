/**
 * App shell boot sequence:
 *   1. first-run setup pending  → /setup.html
 *   2. not signed in            → /login.html?next=…
 *   3. must change password     → /login.html?change=1
 *   4. otherwise show the shell and start the router.
 */
import { api, ApiError } from './core/api.js';
import { $ } from './core/dom.js';
import { createRouter } from './core/router.js';
import { session } from './core/session.js';

const routes = [
  { path: '/', view: () => import('./views/dashboard.js') },
];

async function boot() {
  const setup = await api.get('/setup/status');
  if (setup.setup_required) {
    window.location.replace('/setup.html');
    return;
  }

  const me = await loadCurrentUser();
  if (!me) return;
  session.setMe(me);
  showShell(me);

  const router = createRouter({
    root: $('#view'),
    routes,
    notFound: () => import('./views/not-found.js'),
    onNavigate: highlightNav,
  });
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
    const active = link.dataset.nav === pathname;
    link.classList.toggle('is-active', active);
    if (active) link.setAttribute('aria-current', 'page');
    else link.removeAttribute('aria-current');
  }
}

boot().catch((error) => {
  console.error(error);
  $('[data-boot]').textContent = `Could not start: ${error.message}`;
});
