/**
 * Form helpers: one submit handler with busy state and inline error display.
 *
 *   onSubmit(form, async (values) => { await api.post('/thing', values); });
 *
 * If the handler throws, its message is shown in the form's `[data-form-error]` element.
 *
 * Forms in the HTML pages ship with `method="post"` and a disabled submit button, which this
 * enables once the handler is attached: a click before the script loaded must never submit the
 * form natively (a GET would put the password in the URL).
 */

/** Calls `handler(values)` on submit, where `values` is a plain object of the form fields. */
export function onSubmit(form, handler) {
  for (const button of form.querySelectorAll('[type="submit"]')) button.disabled = false;
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    // Read values before disabling the controls: disabled fields are left out of FormData.
    const values = Object.fromEntries(new FormData(form).entries());
    showError(form, null);
    setBusy(form, true);
    try {
      await handler(values);
    } catch (error) {
      showError(form, error?.message ?? String(error));
    } finally {
      setBusy(form, false);
    }
  });
}

export function showError(form, message) {
  const box = form.querySelector('[data-form-error]');
  if (!box) return;
  box.textContent = message ?? '';
  box.hidden = !message;
}

function setBusy(form, busy) {
  form.setAttribute('aria-busy', String(busy));
  for (const control of form.elements) control.disabled = busy;
  form.querySelector('[type="submit"]')?.classList.toggle('is-busy', busy);
}
