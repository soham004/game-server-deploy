/** First-run setup page: creates the owner account. */
import { api } from '../core/api.js';
import { $ } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';

const form = $('[data-form="setup"]');
const tokenField = $('[data-token-field]');

const status = await api.get('/setup/status');
if (!status.setup_required) {
  window.location.replace('/login.html');
} else {
  tokenField.hidden = !status.token_required;
  if (status.token_required) form.elements.setup_token.focus();
}

onSubmit(form, async ({ username, password, password_confirm, setup_token }) => {
  if (password !== password_confirm) {
    throw new Error('The passwords do not match.');
  }
  await api.post('/setup', { username, password, setup_token: setup_token || null });
  window.location.assign('/');
});
