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

async function redirectIfAlreadySignedIn() {
  try {
    const setup = await api.get('/setup/status');
    if (setup.setup_required) {
      window.location.replace('/setup.html');
      return;
    }
    const me = await api.get('/me');
    if (!me.user.must_change_password) window.location.replace(next);
  } catch {
    // Not signed in: stay on this page.
  }
}

/** Only allow same-site relative paths, so `?next=` can't send users elsewhere. */
function safeNext(raw) {
  return raw && raw.startsWith('/') && !raw.startsWith('//') ? raw : '/';
}
