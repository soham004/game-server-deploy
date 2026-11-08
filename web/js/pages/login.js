/**
 * Sign-in page. Also handles the forced password change after an admin reset
 * (`/login.html?change=1`, or when the login response says `must_change_password`).
 */
import { api } from '../core/api.js';
import { $ } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';

const params = new URLSearchParams(window.location.search);
const next = safeNext(params.get('next'));
const loginForm = $('[data-form="login"]');
const changeForm = $('[data-form="change-password"]');

onSubmit(loginForm, async ({ username, password }) => {
  const { user } = await api.post('/auth/login', { username, password });
  if (user.must_change_password) {
    changeForm.elements.current_password.value = password;
    showChangeForm();
    return;
  }
  window.location.assign(next);
});

onSubmit(changeForm, async ({ current_password, new_password }) => {
  await api.post('/me/password', { current_password, new_password });
  window.location.assign(next);
});

if (params.get('change') === '1') {
  showChangeForm();
} else {
  redirectIfAlreadySignedIn();
}

function showChangeForm() {
  loginForm.hidden = true;
  changeForm.hidden = false;
  changeForm.elements.new_password.focus();
}

// The server sends visitors to the setup page while there is no account yet, so this page only
// has to handle signing in.
async function redirectIfAlreadySignedIn() {
  try {
    const me = await api.get('/me');
    if (!me.user.must_change_password) window.location.replace(next);
  } catch {
    // Not signed in: stay on this page.
  }
}

/**
 * Only a path on this site, so `?next=` can't send users elsewhere. Parsed the way the browser
 * will follow it: `/\evil.test` and `/%5Cevil.test` look like paths but browsers treat the
 * first as `//evil.test`.
 */
function safeNext(raw) {
  if (!raw || !raw.startsWith('/')) return '/';
  const url = new URL(raw, window.location.origin);
  return url.origin === window.location.origin ? url.pathname + url.search + url.hash : '/';
}
