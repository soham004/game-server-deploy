// Static checks of the server-rendered HTML pages. Run: node --test tests/web/

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const pages = ['login.html', 'setup.html'].map((name) => ({
  name,
  html: readFileSync(new URL(`../../web/${name}`, import.meta.url), 'utf8'),
}));

/**
 * Regression (found by the browser test, 2026-11-07): a click on "Sign in" before the page
 * script attached its handler submitted the form natively as a GET, putting the password in the
 * URL. Forms must POST, and their submit buttons start disabled (core/forms.js enables them).
 */
test('password forms never submit natively with GET', () => {
  for (const { name, html } of pages) {
    const forms = html.match(/<form\b[^>]*>/g) ?? [];
    assert.ok(forms.length > 0, `${name} has forms`);
    for (const form of forms) assert.match(form, /method="post"/, `${name}: ${form}`);
    const submits = html.match(/<button\b[^>]*type="submit"[^>]*>/g) ?? [];
    assert.ok(submits.length > 0, `${name} has submit buttons`);
    for (const button of submits) assert.match(button, /\bdisabled\b/, `${name}: ${button}`);
  }
});
