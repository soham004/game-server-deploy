// End-to-end UI test in a real (headless) browser against a running app with a real server.
// Set up with loaders.e2e.mjs, serve with serve.sh (see README.md), then:
//
//   E2E_URL=http://localhost:18739 E2E_SCREENSHOTS=/tmp/shots node --test tests/e2e/ui.e2e.mjs
//
// E2E_SERVER picks the server to drive (default "E2E neoforge"); E2E_USER and E2E_PASSWORD sign in.
// Skipped unless E2E_URL is set.

import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, test } from 'node:test';

import { Browser } from './cdp.mjs';
import { until } from './lib/client.mjs';
import { download, smallModpack } from './lib/small-modpack.mjs';

const BASE = process.env.E2E_URL;
const SERVER = process.env.E2E_SERVER ?? 'E2E neoforge';
const SHOTS = process.env.E2E_SCREENSHOTS;
const skip = BASE ? false : 'set E2E_URL to run the UI end-to-end test';

let browser;
let page;
let serverId;
let failed = false;

/** Runs one test body; on failure saves a screenshot and the page text, and skips the rest. */
function step(name, body) {
  return async (t) => {
    if (failed) {
      t.skip('an earlier step failed');
      return;
    }
    try {
      await body(t);
    } catch (error) {
      failed = true;
      await shot(`FAILED-${name}`).catch(() => {});
      const text = await page.evaluate(() => `${location.href}
${document.body.innerText}`).catch(() => '(no page)');
      throw new Error(`${error.message}
--- page at failure ---
${text.slice(0, 2000)}`);
    }
  };
}

async function shot(name) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot(join(SHOTS, `${name}.png`));
}

before(async () => {
  if (skip) return;
  browser = await Browser.launch({ width: 1280, height: 900 });
  page = await browser.newPage();
});

after(async () => {
  if (page && serverId) {
    // Leave the server stopped whatever happened.
    await page.evaluate((id) => fetch(`/api/v1/instances/${id}/stop`, { method: 'POST', headers: { 'X-CSRF': '1' } }), serverId).catch(() => {});
  }
  await browser?.close();
});

test('signs in (only the sign-in page is open) and lists the servers', { skip }, step('sign-in', async () => {
  // Signed out, a deep link leads to the sign-in page, which brings you back afterwards.
  await page.goto(`${BASE}/users`);
  await page.waitFor(() => location.pathname === '/login.html' && new URLSearchParams(location.search).get('next') === '/users', { message: 'redirect to the login page' });
  const closed = await page.evaluate(async () => Promise.all(['/js/main.js', '/js/views/users.js', '/api/v1/meta', '/api/v1/instances']
    .map(async (path) => `${path} ${(await fetch(path)).status}`)));
  assert.deepEqual(closed, ['/js/main.js 401', '/js/views/users.js 401', '/api/v1/meta 401', '/api/v1/instances 401']);
  // The button is enabled only once the page script handles the form (see core/forms.js).
  await page.waitFor(() => document.querySelector('form[data-form="login"] button[type="submit"]')?.disabled === false, {
    message: 'the sign-in form to be ready',
  });
  await page.fill('#login-username', process.env.E2E_USER ?? 'admin');
  await page.fill('#login-password', process.env.E2E_PASSWORD ?? 'correct horse battery');
  await page.click('form[data-form="login"] button[type="submit"]');
  await page.waitFor(() => location.pathname === '/users' && document.querySelector('.l-page-header__title')?.textContent, { message: 'back on the users page' });

  // `next` never leaves this site (`/\evil.test` is how browsers spell `//evil.test`).
  await page.goto(`${BASE}/login.html?next=/%5Cevil.test/x`);
  await page.waitFor(() => location.pathname === '/' && document.querySelector('.c-server-card'), { message: 'the dashboard, not another site' });
  assert.equal(await page.evaluate(() => location.origin), new URL(BASE).origin);
  await page.waitFor((name) => [...document.querySelectorAll('.c-server-card')].some((c) => c.textContent.includes(name)), {
    args: [SERVER],
    message: `a card for ${SERVER}`,
  });
  await shot('01-dashboard');
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('starts the server and shows a clean, coloured console', { skip, timeout: 180000 }, step('console', async () => {
  await page.click('.c-server-card', SERVER);
  await page.waitFor((name) => document.querySelector('.l-page-header__title')?.textContent === name, {
    args: [SERVER],
    message: 'the server page',
  });
  const software = await page.evaluate(() => document.querySelector('.c-instance-heading__meta .c-badge').textContent);
  assert.match(software, /^(Vanilla|Paper|Purpur|Fabric|Quilt|Forge|NeoForge) \d/, 'loader display name, not its id');
  serverId = await page.evaluate(() => location.pathname.split('/').pop());
  await page.click('.c-actions button', 'Start');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Running', {
    timeout: 150000,
    interval: 1000,
    message: 'the server to be running',
  });

  const console = await page.evaluate(() => {
    const lines = [...document.querySelectorAll('.c-console__line')];
    return {
      count: lines.length,
      text: lines.map((line) => line.textContent).join('\n'),
      coloured: document.querySelectorAll('.c-console__line [class^="ansi-"], .c-console__line [style*="color"]').length,
    };
  });
  assert.ok(console.count > 20, `console has output (${console.count} lines)`);
  assert.ok(!/[\u0000-\u0008\u000b-\u001f\u007f]/.test(console.text), 'no control characters are shown');
  assert.ok(console.text.includes('Done ('), 'the startup finished line is shown');

  await page.fill('.c-console__input', 'list');
  await page.evaluate(() => document.querySelector('.c-console__form').requestSubmit());
  await page.waitFor(() => [...document.querySelectorAll('.c-console__line')].some((l) => l.textContent.includes('players online')), {
    timeout: 15000,
    message: 'the answer to list',
  });
  await shot('02-console-running');
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('edits server.properties visually and in the raw view', { skip, timeout: 60000 }, step('properties', async () => {
  await page.click('.c-tabs__tab', 'Properties');
  await page.waitFor(() => document.querySelector('#prop-max-players'), { message: 'the properties form' });
  const initial = await page.evaluate(() => ({
    difficulty: document.querySelector('#prop-difficulty').value,
    portReadOnly: document.querySelector('#prop-server-port').readOnly,
    sections: [...document.querySelectorAll('.c-props__heading')].map((h) => h.textContent),
  }));
  assert.ok(['peaceful', 'easy', 'normal', 'hard'].includes(initial.difficulty));
  assert.equal(initial.portReadOnly, true, 'the app-managed port is read-only');
  assert.ok(initial.sections.includes('General') && initial.sections.includes('World'), initial.sections.join());
  await shot('03-properties-form');

  // Two valid changes: the footer counts them and saving reports that a restart is needed.
  await page.fill('#prop-max-players', '7');
  await page.fill('#prop-difficulty', initial.difficulty === 'hard' ? 'normal' : 'hard');
  await page.waitFor(() => document.querySelector('.c-props__footer-text')?.textContent === '2 unsaved changes');
  await page.click('.c-props__footer button', 'Save');
  await page.waitFor(() => [...document.querySelectorAll('.c-toast')].some((t) => t.textContent.includes('Restart the server')), {
    message: 'the saved toast asking for a restart',
  });
  const saved = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/properties`)).json()).values, serverId);
  assert.equal(saved.find((kv) => kv.key === 'max-players').value, '7');
  await shot('04-properties-saved');

  // An invalid value is refused and marked on its field.
  await page.click('label.c-props__toggle input');
  await page.fill('#prop-view-distance', '100');
  await page.click('.c-props__footer button', 'Save');
  await page.waitFor(() => document.querySelector('.c-props__field.is-invalid .c-props__error')?.textContent.includes('view-distance'), {
    message: 'the field error',
  });
  await shot('05-properties-invalid');
  await page.click('.c-props__footer button', 'Discard');

  // The raw view shows the saved file.
  await page.click('.c-segmented button', 'Raw file');
  const raw = await page.evaluate(() => document.querySelector('.c-props__raw').value);
  assert.ok(raw.includes('max-players=7'), 'raw text has the saved value');
  await shot('06-properties-raw');

  // Put max-players back.
  await page.click('.c-segmented button', 'Form');
  await page.fill('#prop-max-players', '20');
  await page.click('.c-props__footer button', 'Save');
  await page.waitFor(() => !document.querySelector('.c-props__footer') || document.querySelector('.c-props__footer').hidden);
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('manages files: folders, upload, edit, rename, compress, extract, delete', { skip, timeout: 90000 }, step('files', async () => {
  const rowNames = () => page.evaluate(() => [...document.querySelectorAll('.c-files__row:not(.is-parent)')].map((r) => r.dataset.path));
  const waitForRow = (path) => page.waitFor((p) => [...document.querySelectorAll('.c-files__row')].some((r) => r.dataset.path === p), { args: [path], message: `a row for ${path}` });
  const waitForNoRow = (path) => page.waitFor((p) => ![...document.querySelectorAll('.c-files__row')].some((r) => r.dataset.path === p), { args: [path], message: `no row for ${path}` });
  const rowButton = (path, label) => page.evaluate((p, l) => {
    const row = [...document.querySelectorAll('.c-files__row')].find((r) => r.dataset.path === p);
    [...row.querySelectorAll('button')].find((b) => b.textContent.includes(l)).click();
    return true;
  }, path, label);

  // Leftovers from an interrupted earlier run would make the steps below fail.
  await page.evaluate(async (id) => {
    for (const path of ['e2e-folder', 'e2e.zip', 'e2e-extracted']) {
      await fetch(`/api/v1/instances/${id}/files/delete`, {
        method: 'POST',
        headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' },
        body: JSON.stringify({ paths: [path] }),
      });
    }
  }, serverId);

  await page.click('.c-tabs__tab', 'Files');
  await waitForRow('server.properties');
  assert.ok((await rowNames()).includes('eula.txt'));

  // New folder, then upload a real file from disk into it.
  page.answerDialogs('e2e-folder');
  await page.click('.c-files__toolbar button', 'New folder');
  await waitForRow('e2e-folder');
  await page.click('.c-files__row[data-path="e2e-folder"] .c-files__name');
  await page.waitFor(() => document.querySelector('.c-files__crumbs')?.textContent.includes('e2e-folder'));
  const upload = join(tmpdir(), 'gsd-e2e-upload.txt');
  writeFileSync(upload, 'uploaded from the browser test\n');
  await page.setFiles('[data-upload-input]', [upload]);
  await waitForRow('e2e-folder/gsd-e2e-upload.txt');

  // Open it in the editor, change it, save with the button.
  await page.click('.c-files__row[data-path="e2e-folder/gsd-e2e-upload.txt"] .c-files__name');
  await page.waitFor(() => document.querySelector('.c-editor:not([hidden]) .c-editor__text')?.value.startsWith('uploaded'));
  const editorHeight = await page.evaluate(() => document.querySelector('.c-editor__text').getBoundingClientRect().height);
  assert.ok(editorHeight > 300, `the editor is tall enough to edit in (${editorHeight}px)`);
  await page.fill('.c-editor__text', 'edited in the browser\n');
  await page.click('.c-editor__header button', 'Save');
  await page.waitFor(() => document.querySelector('.c-editor__status')?.textContent === 'Saved', { message: 'the saved status' });
  await shot('08-files-editor');
  const saved = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/files/text?path=e2e-folder/gsd-e2e-upload.txt`)).json()).content, serverId);
  assert.equal(saved, 'edited in the browser\n');
  await page.click('.c-editor__header button', 'Close');

  // Rename, then go back up and compress the folder.
  page.answerDialogs('renamed.txt');
  await rowButton('e2e-folder/gsd-e2e-upload.txt', 'Rename');
  await waitForRow('e2e-folder/renamed.txt');
  await page.click('.c-files__crumb', 'server');
  await waitForRow('e2e-folder');
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('.c-files__row')].find((r) => r.dataset.path === 'e2e-folder');
    row.querySelector('input[type="checkbox"]').click();
  });
  page.answerDialogs('e2e.zip');
  await page.click('.c-files__selection button', 'Compress');
  await waitForRow('e2e.zip');

  // Extract the archive next to it.
  page.answerDialogs('e2e-extracted');
  await rowButton('e2e.zip', 'Extract');
  await waitForRow('e2e-extracted');
  const extracted = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/files/text?path=e2e-extracted/e2e-folder/renamed.txt`)).json()).content, serverId);
  assert.equal(extracted, 'edited in the browser\n');
  await shot('09-files-list');

  // Downloads are attachments of the right file. (A text file: on the development machine the
  // browser itself blocks every .zip response with an empty 204, even from unrelated servers;
  // zip downloads are covered by the API tests.)
  const download = await page.evaluate(async (id) => {
    const response = await fetch(`/api/v1/instances/${id}/files/download?path=e2e-extracted/e2e-folder/renamed.txt`);
    return {
      status: response.status,
      type: response.headers.get('content-type'),
      disposition: response.headers.get('content-disposition'),
      text: await response.text(),
    };
  }, serverId);
  assert.equal(download.status, 200);
  assert.equal(download.type, 'application/octet-stream');
  assert.match(download.disposition, /^attachment;.*renamed\.txt/);
  assert.equal(download.text, 'edited in the browser\n');

  const toasts = await page.evaluate(() => document.querySelectorAll('.c-toast').length);
  assert.ok(toasts <= 3, `at most three toasts at once (${toasts})`);

  // Clean up: delete all three (confirm).
  for (const path of ['e2e-folder', 'e2e.zip', 'e2e-extracted']) {
    page.answerDialogs(true);
    await rowButton(path, 'Delete');
    await waitForNoRow(path);
  }
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('changes settings through the form', { skip, timeout: 30000 }, step('settings', async () => {
  await page.click('.c-tabs__tab', 'Settings');
  await page.waitFor(() => document.querySelector('#settings-max_memory_mb'), { message: 'the settings form' });
  const before = await page.evaluate(() => ({
    max: document.querySelector('#settings-max_memory_mb').value,
    args: document.querySelector('#settings-extra_jvm_args').value,
  }));
  await page.fill('#settings-max_memory_mb', '2560');
  await page.fill('#settings-extra_jvm_args', `${before.args}\n-De2e.flag=1`.trim());
  await page.click('.c-settings button[type="submit"]');
  await page.waitFor(() => [...document.querySelectorAll('.c-toast')].some((t) => /Saved|saved/.test(t.textContent)), { message: 'the saved toast' });
  const saved = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/settings`)).json()).java, serverId);
  assert.equal(saved.max_memory_mb, 2560);
  assert.ok(saved.extra_jvm_args.includes('-De2e.flag=1'));
  await shot('10-settings');

  // An invalid value is refused with a message.
  await page.fill('#settings-max_memory_mb', '100');
  await page.click('.c-settings button[type="submit"]');
  await page.waitFor(() => [...document.querySelectorAll('.c-toast')].some((t) => t.textContent.includes('Memory must be')
    || t.textContent.includes('memory must be')), { message: 'the validation message' });

  // Put things back.
  await page.evaluate(async ({ id, before }) => {
    await fetch(`/api/v1/instances/${id}/settings`, {
      method: 'PATCH',
      headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' },
      body: JSON.stringify({ max_memory_mb: Number(before.max), extra_jvm_args: before.args.split('\n').filter(Boolean) }),
    });
  }, { id: serverId, before });
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('backs up the running server and restores the world', { skip, timeout: 420000 }, step('backups', async () => {
  const api = (path, init = {}) => page.evaluate(async ({ path, init }) => {
    const response = await fetch(`/api/v1${path}`, { ...init, headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' } });
    return response.status === 204 ? null : response.json();
  }, { path, init });
  const marker = 'world/e2e-after-backup.txt';
  const markerExists = async () => (await api(`/instances/${serverId}/files?path=world`)).entries.some((e) => e.name === 'e2e-after-backup.txt');
  const rowWithNote = (note) => page.waitFor((n) => [...document.querySelectorAll('.c-backups__table tbody tr')].some((r) => r.textContent.includes(n)), {
    args: [note], timeout: 180000, interval: 500, message: `a backup noted "${note}"`,
  });

  await page.click('.c-tabs__tab', 'Backups');
  await page.waitFor(() => document.querySelector('.c-backups__summary')?.textContent.includes('kept'), { message: 'the backups tab' });
  await page.fill('#backup-scope', 'world');
  await page.fill('#backup-note', 'e2e checkpoint');
  await page.click('.c-backups__create button[type="submit"]');
  await rowWithNote('e2e checkpoint');
  const backups = (await api(`/instances/${serverId}/backups`)).backups;
  const checkpoint = backups.find((b) => b.note === 'e2e checkpoint');
  assert.equal(checkpoint.scope, 'world');
  assert.ok(checkpoint.size_bytes > 100_000, `a real world was archived (${checkpoint.size_bytes} bytes)`);
  const consoleText = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/console`)).json()).console.map((l) => l.text).join('\n'), serverId);
  assert.ok(consoleText.includes('[backup] > save-off') && consoleText.includes('Saved the game') && consoleText.includes('[backup] > save-on'),
    'the running world was paused, flushed and resumed');
  await shot('14-backups');

  // Something changes after the backup; restoring brings the world back as it was.
  await api(`/instances/${serverId}/files/text`, { method: 'PUT', body: JSON.stringify({ path: marker, content: 'made after the backup' }) });
  assert.ok(await markerExists());
  page.answerDialogs(true);
  await page.evaluate((id) => {
    const row = document.querySelector(`.c-backups__table tr[data-backup="${id}"]`);
    [...row.querySelectorAll('button')].find((b) => b.textContent === 'Restore').click();
  }, checkpoint.id);
  await page.waitFor(() => [...document.querySelectorAll('.c-toast')].some((t) => t.textContent.includes('restored')), {
    timeout: 300000, interval: 1000, message: 'the restore to finish',
  });
  assert.equal(await markerExists(), false, 'the world is back as it was');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Running', {
    timeout: 150000, interval: 1000, message: 'the server to run again (it was running before)',
  });
  await page.waitFor(() => [...document.querySelectorAll('.c-backups__kind')].some((b) => b.textContent === 'Before a restore'), { message: 'the safety backup row' });
  await shot('15-restored');

  // Clean up: delete both through the UI.
  for (const backup of (await api(`/instances/${serverId}/backups`)).backups) {
    page.answerDialogs(true);
    await page.evaluate((id) => {
      const row = document.querySelector(`.c-backups__table tr[data-backup="${id}"]`);
      [...row.querySelectorAll('button')].find((b) => b.textContent === 'Delete').click();
    }, backup.id);
    await page.waitFor((id) => !document.querySelector(`.c-backups__table tr[data-backup="${id}"]`), { args: [backup.id], message: 'the row to go' });
  }
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('schedules: from a template, run now, by hand, disable, history', { skip, timeout: 120000 }, step('schedules', async () => {
  const item = (name) => `.c-schedule[data-schedule="${name}"]`;
  const idOf = (name) => page.evaluate((n) => [...document.querySelectorAll('.c-schedule')]
    .find((li) => li.querySelector('.c-schedule__title strong')?.textContent === n)?.dataset.schedule, name);
  const clickIn = (id, label) => page.evaluate((i, l) => {
    [...document.querySelector(`.c-schedule[data-schedule="${i}"]`).querySelectorAll('button')].find((b) => b.textContent === l).click();
  }, id, label);

  await page.click('.c-tabs__tab', 'Schedules');
  await page.waitFor(() => document.querySelector('.c-schedules__toolbar select'), { message: 'the schedules toolbar' });

  // From a template: the editor opens filled in; change the message and save.
  await page.fill('.c-schedules__toolbar select', 'announce');
  await page.waitFor(() => document.querySelector('.c-schedule-editor'), { message: 'the editor' });
  assert.equal(await page.evaluate(() => document.querySelector('.c-schedule-editor [name="name"]').value), 'Announcement');
  await page.fill('.c-schedule-editor [name="message-0"]', 'e2e hello from a schedule');
  await page.click('.c-schedule-editor button[type="submit"]');
  await page.waitFor(() => [...document.querySelectorAll('.c-schedule')].some((li) => li.textContent.includes('Every 30 minutes')), { message: 'the saved schedule' });
  const announcement = await idOf('Announcement');

  // Run now: the result shows on the item, the message in the console.
  await clickIn(announcement, 'Run now');
  await page.waitFor((s) => document.querySelector(s)?.querySelector('.c-pill.is-succeeded'), { args: [item(announcement)], timeout: 30000, message: 'the run to succeed' });
  const consoleText = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/console`)).json()).console.map((l) => l.text).join('\n'), serverId);
  assert.ok(consoleText.includes('[schedule Announcement] > say e2e hello from a schedule'), 'the message reached the server');

  // By hand: every day at 04:00, restart with warnings.
  await page.click('.c-schedules__toolbar button', 'New schedule');
  await page.fill('.c-schedule-editor [name="name"]', 'E2E nightly restart');
  await page.fill('.c-schedule-editor [name="when"]', 'daily');
  await page.waitFor(() => document.querySelector('.c-schedule-editor [name="daily_time"]'), { message: 'the time field' });
  await page.fill('.c-schedule-editor [name="daily_time"]', '04:00');
  await page.fill('.c-schedule-editor [aria-label="Action 1"]', 'restart');
  await page.waitFor(() => document.querySelector('.c-schedule-editor [name="warnings-0"]'), { message: 'the warnings field' });
  // Nonsense is refused in the browser, with a reason.
  await page.fill('.c-schedule-editor [name="warnings-0"]', 'soon');
  await page.click('.c-schedule-editor button[type="submit"]');
  await page.waitFor(() => [...document.querySelectorAll('.c-toast')].some((t) => t.textContent.includes('not a time')), { message: 'the warning error' });
  await page.fill('.c-schedule-editor [name="warnings-0"]', '10m, 1m');
  await page.click('.c-schedule-editor button[type="submit"]');
  await page.waitFor(() => [...document.querySelectorAll('.c-schedule')].some((li) => li.textContent.includes('E2E nightly restart')), { message: 'the nightly schedule' });
  const nightly = await idOf('E2E nightly restart');
  const nightlyText = await page.evaluate((s) => document.querySelector(s).textContent, item(nightly));
  assert.ok(nightlyText.includes('04:00') && nightlyText.includes('Restart (warnings 10m, 1m)') && nightlyText.includes('next in'), nightlyText);
  await shot('16-schedules');

  // Disable it; open the announcement's history.
  await clickIn(nightly, 'Disable');
  await page.waitFor((s) => document.querySelector(s)?.querySelector('.c-badge')?.textContent === 'Disabled', { args: [item(nightly)], message: 'the disabled badge' });
  await clickIn(announcement, 'History');
  await page.waitFor((s) => document.querySelector(`${s} .c-schedule__runs`)?.textContent.includes('Run now'), { args: [item(announcement)], message: 'the run history' });
  await shot('17-schedule-history');

  // Clean up through the UI.
  for (const id of [announcement, nightly]) {
    page.answerDialogs(true);
    await clickIn(id, 'Delete');
    await page.waitFor((s) => !document.querySelector(s), { args: [item(id)], message: 'the schedule to go' });
  }
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('mods: search Modrinth, install, update, turn off and on, remove', { skip, timeout: 300000 }, step('mods', async () => {
  const rowFor = (name) => page.evaluate((n) => [...document.querySelectorAll('.c-content__table tbody tr[data-file]')]
    .find((tr) => tr.querySelector('.c-content__name')?.textContent === n)?.dataset.file ?? null, name);
  const clickRow = (file, label) => page.evaluate((f, l) => {
    const row = document.querySelector(`.c-content__table tr[data-file="${CSS.escape(f)}"]`);
    [...row.querySelectorAll('button')].find((b) => b.textContent === l).click();
  }, file, label);

  await page.click('.c-tabs__tab', 'Mods');
  await page.waitFor(() => document.querySelector('.c-content__summary')?.textContent, { message: 'the mods tab' });
  await page.waitFor(() => document.querySelectorAll('.c-content-result').length > 0, { timeout: 30000, message: 'the first results from Modrinth' });
  // No button, no Enter: the search runs by itself once typing stops.
  assert.equal(await page.evaluate(() => document.querySelectorAll('.c-content-browser__search button').length), 0, 'no search button');
  await page.fill('.c-content-browser__search input', 'lithium');
  await page.waitFor(() => [...document.querySelectorAll('.c-content-result__title')].some((a) => a.textContent === 'Lithium'), { timeout: 30000, message: 'Lithium in the results' });
  // "Install" and "Other versions", or one disabled "Installed", for the Lithium result.
  const lithiumActions = (expected, message) => page.waitFor((want) => {
    const result = [...document.querySelectorAll('.c-content-result')].find((li) => li.querySelector('.c-content-result__title').textContent === 'Lithium');
    const buttons = [...(result?.querySelectorAll('.c-content-browser__actions button') ?? [])];
    return buttons.map((b) => `${b.textContent}${b.disabled ? ' (disabled)' : ''}`).join(', ') === want;
  }, { args: [expected], timeout: 30000, message });
  await lithiumActions('Install, Other versions', 'Lithium can be installed');

  page.answerDialogs(true); // the plan
  await page.evaluate(() => {
    const result = [...document.querySelectorAll('.c-content-result')].find((li) => li.querySelector('.c-content-result__title').textContent === 'Lithium');
    [...result.querySelectorAll('button')].find((b) => b.textContent === 'Install').click();
  });
  await page.waitFor(() => [...document.querySelectorAll('.c-content__name')].some((n) => n.textContent === 'Lithium'), { timeout: 120000, message: 'Lithium installed' });
  await lithiumActions('Installed (disabled), Other versions', 'Lithium marked installed in the results');
  await page.waitFor(() => !document.querySelector('.c-content > .c-banner').hidden, { message: 'the restart notice (the server runs)' });
  await shot('22-mods');

  const file = await rowFor('Lithium');
  await clickRow(file, 'Turn off');
  await page.waitFor((f) => document.querySelector(`.c-content__table tr[data-file="${CSS.escape(f)}"]`)?.classList.contains('is-disabled'), { args: [file], message: 'turned off' });
  await clickRow(file, 'Turn on');
  await page.waitFor((f) => !document.querySelector(`.c-content__table tr[data-file="${CSS.escape(f)}"]`)?.classList.contains('is-disabled'), { args: [file], message: 'turned on' });
  await page.waitFor(() => document.querySelector('.c-content-updates__status')?.textContent.includes('up to date'), { timeout: 30000, message: 'the newest Lithium is up to date' });

  // Back to an older Lithium through "Other versions" (it replaces the installed file): the updates
  // panel then offers the newest again.
  const versions = await page.evaluate(async (id) => (await fetch(`/api/v1/instances/${id}/content/projects/gvQqBUqZ/versions`)).json(), serverId);
  assert.ok(versions.length >= 2, `Lithium has an older version for this server: ${versions.length}`);
  const inLithium = (selector) => `.c-content-result:has(a.c-content-result__title[href$="/lithium"]) ${selector}`;
  await page.click(inLithium('.c-content-browser__actions button'), 'Other versions');
  await page.waitFor((s) => document.querySelector(s), { args: [inLithium('.c-content-result__versions select')], message: 'the version picker' });
  await page.fill(inLithium('.c-content-result__versions select'), versions[1].id);
  page.answerDialogs(true); // the plan: replacing the newest
  await page.click(inLithium('.c-content-result__versions button'), 'Install this version');
  await page.waitFor((f) => document.querySelector(`.c-content__table tr[data-file="${CSS.escape(f)}"]`), { args: [versions[1].file.filename], timeout: 120000, message: 'the older Lithium installed' });
  assert.equal(await page.evaluate(() => document.querySelectorAll('.c-content__table tbody tr[data-file]').length), 1, 'it replaced the newest, not added a second copy');
  await lithiumActions('Installed (disabled), Other versions', 'still marked installed after going back');
  await page.waitFor((label) => [...document.querySelectorAll('.c-content-updates__item')].some((li) => li.textContent.includes('Lithium') && li.textContent.includes(label)),
    { args: [`${versions[1].version_number} → ${versions[0].version_number}`], timeout: 60000, message: 'Lithium offered as an update, both versions as Modrinth numbers them' });
  await shot('23-mods-update');
  await page.click('.c-content-updates__actions button', 'Update selected');
  await page.waitFor((f) => document.querySelector(`.c-content__table tr[data-file="${CSS.escape(f)}"]`), { args: [versions[0].file.filename], timeout: 180000, message: 'the newest Lithium back after the update' });
  await page.waitFor(() => document.querySelector('.c-content-updates__status')?.textContent.includes('up to date'), { timeout: 30000, message: 'up to date after updating' });
  const backups = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/backups`)).json()).backups.map((b) => b.kind), serverId);
  assert.ok(backups.includes('pre_update'), `a backup before updating: ${backups}`);

  page.answerDialogs(true);
  await clickRow(versions[0].file.filename, 'Remove');
  await page.waitFor(() => !document.querySelector('.c-content__table tr[data-file]'), { message: 'nothing installed again' });
  await lithiumActions('Install, Other versions', 'Lithium can be installed again after removing it');
  await page.waitFor(() => !document.querySelector('.c-content-updates'), { message: 'no updates panel with nothing installed' });
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('mods: tick several, then install them together with their dependencies', { skip, timeout: 300000 }, step('mods-batch', async () => {
  // Results and install-list entries by Modrinth slug (the result's link ends with it).
  const resultFor = (slug) => `.c-content-result:has(a.c-content-result__title[href$="/${slug}"])`;
  const listed = () => page.evaluate(() => [...document.querySelectorAll('.c-install-list .c-content__name')].map((t) => t.textContent));
  /** Every visible result laid out in its columns: checkbox, icon, text, then the buttons to the right. */
  const resultsInColumns = () => page.evaluate(() => [...document.querySelectorAll('.c-content-result:not([hidden])')].every((row) => {
    const box = (selector) => row.querySelector(selector).getBoundingClientRect();
    const [pick, icon, text, actions] = ['.c-content-result__pick', '.c-content-result__icon', '.c-content-result__text', '.c-content-browser__actions'].map(box);
    return pick.right <= icon.left && icon.right <= text.left && text.right <= actions.left && actions.top < text.bottom;
  }));
  const searchFor = async (query, slug) => {
    await page.fill('.c-content-browser__search input', query);
    await page.waitFor((s) => document.querySelector(s), { args: [resultFor(slug)], timeout: 30000, message: `${slug} in the results` });
  };
  const tick = (slug) => page.evaluate((s) => document.querySelector(`${s} .c-content-result__pick input`).click(), resultFor(slug));

  // REI needs Cloth Config and Architectury on NeoForge; FerriteCore needs nothing.
  await searchFor('roughly enough items', 'rei');
  await tick('rei');
  assert.ok(await page.evaluate((s) => document.querySelector(s).hidden, resultFor('rei')), 'a ticked result moves to the list');
  await searchFor('ferritecore', 'ferrite-core');
  await tick('ferrite-core');
  assert.equal((await listed()).length, 2, 'the list keeps what was ticked across searches');
  assert.equal(await page.evaluate(() => document.querySelector('.c-install-list__actions .c-button--primary').textContent), 'Install 2 mods');

  // Unticking puts it back among the results; tick it again.
  await page.click('.c-install-list tbody tr:last-child button', 'Remove');
  assert.equal((await listed()).length, 1);
  assert.equal(await page.evaluate((s) => document.querySelector(s).hidden, resultFor('ferrite-core')), false, 'back among the results');
  await tick('ferrite-core');
  assert.ok(await resultsInColumns(), 'results keep their columns');
  // The list sits between the installed list and the search, as a table like the installed one.
  assert.ok(await page.evaluate(() => {
    const list = document.querySelector('.c-install-list');
    return list.compareDocumentPosition(document.querySelector('.c-content-browser')) & Node.DOCUMENT_POSITION_FOLLOWING
      && list.querySelector('table.c-content__table thead th')?.textContent === 'Name';
  }), 'a table above the search');
  await shot('24-mods-install-list');

  page.answerDialogs(true); // the plan for both
  await page.click('.c-install-list__actions .c-button--primary', 'Install 2 mods');
  await page.waitFor(() => document.querySelector('.c-install-list').hidden, { message: 'the list empties once installing' });
  const plan = page.dialogs.at(-1);
  assert.match(plan, /needed by Roughly Enough Items/, `the dependencies are in the plan:\n${plan}`);
  await page.waitFor(() => document.querySelectorAll('.c-content__table tbody tr[data-file]').length === 4, { timeout: 180000, message: 'both mods and their two dependencies installed' });
  // Jars name themselves; the index says which project each one is.
  const projects = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/content`)).json()).items.map((item) => item.source?.project_id), serverId);
  for (const dependency of ['9s6osm5g', 'lhGA9TYQ']) assert.ok(projects.includes(dependency), `Cloth Config and Architectury: ${projects}`);
  await page.waitFor((s) => document.querySelector(`${s} .c-content-browser__actions button`)?.textContent === 'Installed', { args: [resultFor('ferrite-core')], message: 'FerriteCore marked installed' });
  assert.ok(await resultsInColumns(), 'installed results keep their columns');
  await shot('25-mods-installed-results');

  // Leave the server as it was.
  const files = await page.evaluate(() => [...document.querySelectorAll('.c-content__table tbody tr[data-file]')].map((tr) => tr.dataset.file));
  for (const file of files) {
    const status = await page.evaluate(async ({ id, f }) => (await fetch(`/api/v1/instances/${id}/content/files/${encodeURIComponent(f)}`, { method: 'DELETE', headers: { 'X-CSRF': '1' } })).status, { id: serverId, f: file });
    assert.equal(status, 204, file);
  }
  await page.waitFor(() => !document.querySelector('.c-content__table tr[data-file]'), { message: 'nothing installed again' });
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('plugins: choose Hangar or Modrinth, tick from both, install', { skip, timeout: 300000 }, step('plugins-sources', async () => {
  const paper = await page.evaluate(async () => {
    const instances = await (await fetch('/api/v1/instances')).json();
    return instances.find((instance) => instance.name === 'E2E paper')?.id ?? null;
  });
  assert.ok(paper, 'loaders.e2e.mjs made "E2E paper"');
  const api = (method, path) => page.evaluate(async ({ m, p }) => {
    const response = await fetch(p, { method: m, headers: { 'X-CSRF': '1' } });
    return response.status === 204 ? null : response.json();
  }, { m: method, p: `/api/v1/instances/${paper}/content${path}` });
  const removeEverything = async () => {
    for (const item of (await api('GET', '')).items) await api('DELETE', `/files/${encodeURIComponent(item.file)}`);
  };
  await removeEverything();

  /** Searches and waits for `title` among results that all link to `site`: answers from the source chosen before don't count. */
  const searchFor = async (query, title, site) => {
    await page.fill('.c-content-browser__search input', query);
    await page.waitFor((t, s) => {
      const links = [...document.querySelectorAll('.c-content-result__title')];
      return links.some((a) => a.textContent === t) && links.every((a) => a.href.startsWith(s));
    }, { args: [title, site], timeout: 30000, message: `${title} in the results from ${site}` });
  };
  const tick = (title) => page.evaluate((t) => {
    const row = [...document.querySelectorAll('.c-content-result')].find((li) => li.querySelector('.c-content-result__title').textContent === t);
    row.querySelector('.c-content-result__pick input').click();
  }, title);

  try {
    await page.goto(`${BASE}/servers/${paper}`);
    await page.waitFor(() => [...document.querySelectorAll('.c-tabs__tab')].some((tab) => tab.textContent === 'Plugins'), { message: 'the server page' });
    await page.click('.c-tabs__tab', 'Plugins');
    await page.waitFor(() => document.querySelectorAll('.c-content-result').length > 0, { timeout: 30000, message: 'results' });
    const picker = '.c-content-browser__search select[name="content_source"]';
    assert.equal(await page.evaluate((s) => document.querySelector(s).hidden, picker), false, 'the picker shows on a plugin server');
    assert.deepEqual(await page.evaluate((s) => [...document.querySelector(s).options].map((o) => o.textContent), picker), ['Modrinth', 'Hangar']);

    // Hangar's results link to Hangar, and the search box says where it looks.
    await page.fill(picker, 'hangar');
    await searchFor('viabackwards', 'ViaBackwards', 'https://hangar.papermc.io/');
    assert.equal(await page.evaluate(() => document.querySelector('.c-content-browser__search input').placeholder), 'Search Hangar');
    await page.waitFor(() => document.querySelector('.c-content-result img.c-content-result__icon')?.naturalWidth > 0, { timeout: 30000, message: 'an icon from Hangar loads' });
    await tick('ViaBackwards');
    await shot('26-plugins-hangar');

    // The list keeps it while searching the other source; Chunky comes from there.
    await page.fill(picker, 'modrinth');
    await searchFor('chunky', 'Chunky', 'https://modrinth.com/');
    await tick('Chunky');
    assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.c-install-list tbody td:nth-child(3)')].map((td) => td.textContent)), ['Hangar', 'Modrinth'], 'From says which source');

    page.answerDialogs(true); // the plan for both
    await page.click('.c-install-list__actions .c-button--primary', 'Install 2 plugins');
    await page.waitFor(() => document.querySelector('.c-install-list').hidden, { message: 'the list empties once installing' });
    assert.match(page.dialogs.at(-1), /ViaVersion .*\(needed by ViaBackwards\)/, 'the Hangar dependency is in the plan');
    await page.waitFor(() => document.querySelectorAll('.c-content__table tbody tr[data-file]').length === 3, { timeout: 180000, message: 'three plugins installed' });
    const from = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.c-content__table tbody tr[data-file]')]
      .map((tr) => [tr.querySelector('.c-content__name').textContent, tr.querySelector('td:nth-child(3)').textContent])));
    assert.deepEqual(from, { Chunky: 'Modrinth', ViaBackwards: 'Hangar', ViaVersion: 'Hangar' });
    await shot('27-plugins-installed');

    // Back on Hangar, what came from Hangar says Installed.
    await page.fill(picker, 'hangar');
    await searchFor('viaversion', 'ViaVersion', 'https://hangar.papermc.io/');
    await page.waitFor(() => {
      const row = [...document.querySelectorAll('.c-content-result')].find((li) => li.querySelector('.c-content-result__title').textContent === 'ViaVersion');
      return row?.querySelector('.c-content-browser__actions button')?.textContent === 'Installed';
    }, { timeout: 30000, message: 'ViaVersion marked installed on Hangar' });
    assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
  } finally {
    await removeEverything();
    await page.goto(`${BASE}/servers/${serverId}`);
    await page.waitFor(() => document.querySelector('.c-tabs__tab'), { message: 'back on the first server' });
  }
}));

test('curseforge: key in settings, install, a file to download by hand', { skip, timeout: 180000 }, step('curseforge', async () => {
  // Against the stand-in serve.sh starts (lib/mock-curseforge.mjs): the suite has no real key.
  const paper = await page.evaluate(async () => (await (await fetch('/api/v1/instances')).json()).find((instance) => instance.name === 'E2E paper')?.id ?? null);
  assert.ok(paper, 'loaders.e2e.mjs made "E2E paper"');
  const call = (method, path) => page.evaluate(async ({ m, p }) => {
    const response = await fetch(`/api/v1${p}`, { method: m, headers: { 'X-CSRF': '1' } });
    return response.status === 204 ? null : response.json();
  }, { m: method, p: path });
  const cleanUp = async () => {
    for (const item of (await call('GET', `/instances/${paper}/content`)).items) await call('DELETE', `/instances/${paper}/content/files/${encodeURIComponent(item.file)}`);
    await call('DELETE', '/settings/curseforge-key');
  };
  const status = () => page.evaluate(() => document.querySelector('[data-curseforge-status]')?.textContent ?? '');
  const inResult = (title, label) => page.evaluate((t, l) => {
    const row = [...document.querySelectorAll('.c-content-result')].find((li) => li.querySelector('.c-content-result__title').textContent === t);
    [...row.querySelectorAll('button')].find((b) => b.textContent === l).click();
  }, title, label);
  await cleanUp();

  try {
    await page.click('.c-nav__link', 'Settings');
    await page.waitFor(() => document.querySelector('[data-curseforge-status]')?.textContent.startsWith('No key'), { message: 'the settings page' });

    await page.fill('#curseforge-key', 'a-wrong-key');
    await page.click('.c-form__actions button', 'Save');
    await page.waitFor(() => document.querySelector('[data-form-error]:not([hidden])')?.textContent.includes('accept'), { message: 'a wrong key refused' });
    assert.match(await status(), /^No key/);

    await page.fill('#curseforge-key', 'e2e-curseforge-key');
    await page.click('.c-form__actions button', 'Save');
    await page.waitFor(() => document.querySelector('[data-curseforge-status]').textContent.startsWith('A key is saved'), { message: 'the key saved' });
    assert.equal(await page.evaluate(() => document.querySelector('#curseforge-key').value), '', 'the field empties');
    assert.ok(!(await page.evaluate(() => document.body.innerHTML)).includes('e2e-curseforge-key'), 'the key is not on the page');
    await shot('28-settings');

    await page.goto(`${BASE}/servers/${paper}`);
    await page.waitFor(() => [...document.querySelectorAll('.c-tabs__tab')].some((tab) => tab.textContent === 'Plugins'), { message: 'the server page' });
    await page.click('.c-tabs__tab', 'Plugins');
    const picker = '.c-content-browser__search select[name="content_source"]';
    await page.waitFor((s) => document.querySelector(s)?.options.length === 3, { args: [picker], message: 'CurseForge offered' });
    await page.fill(picker, 'curseforge');
    await page.waitFor(() => [...document.querySelectorAll('.c-content-result__title')].some((a) => a.textContent === 'E2E CF Plugin'), { timeout: 30000, message: 'CurseForge results' });

    page.answerDialogs(true); // the plan
    await inResult('E2E CF Plugin', 'Install');
    await page.waitFor(() => [...document.querySelectorAll('.c-content__name')].some((n) => n.textContent === 'E2E CF Plugin'), { timeout: 60000, message: 'the plugin installed' });
    assert.equal(await page.evaluate(() => document.querySelector('.c-content__table tr[data-file="e2e-cf-plugin.jar"] td:nth-child(3)').textContent), 'CurseForge');

    page.answerDialogs(true); // the plan: this one can't be downloaded
    await inResult('E2E CF Locked', 'Install');
    await page.waitFor(() => !document.querySelector('.c-manual')?.hidden, { timeout: 30000, message: 'the file to download by hand' });
    assert.match(page.dialogs.at(-1), /only allow downloading by hand[^]*E2E CF Locked/, page.dialogs.at(-1));
    assert.equal(
      await page.evaluate(() => document.querySelector('.c-manual__item a').href),
      'https://www.curseforge.com/minecraft/bukkit-plugins/e2e-cf-locked/files/9102',
    );
    await shot('29-download-by-hand');

    // The file the user downloaded: uploaded, recognised, and tracked like the others.
    const folder = mkdtempSync(join(tmpdir(), 'gsd-e2e-'));
    const downloaded = join(folder, 'e2e-cf-locked.jar');
    writeFileSync(downloaded, 'what the author handed out');
    await page.setFiles('input[name="manual_files"]', [downloaded]);
    await page.waitFor(() => document.querySelector('.c-manual').hidden, { timeout: 30000, message: 'nothing left to download' });
    await page.waitFor(() => document.querySelector('.c-content__table tr[data-file="e2e-cf-locked.jar"] td:nth-child(3)')?.textContent === 'CurseForge', { timeout: 30000, message: 'the upload is tracked' });
    rmSync(folder, { recursive: true });
    await shot('30-uploaded');

    // Removing the key turns the source off; what is installed stays.
    await page.goto(`${BASE}/settings`);
    await page.waitFor(() => document.querySelector('[data-curseforge-status]')?.textContent.startsWith('A key is saved'), { message: 'settings again' });
    page.answerDialogs(true);
    await page.click('.c-form__actions button', 'Remove the key');
    await page.waitFor(() => document.querySelector('[data-curseforge-status]').textContent.startsWith('No key'), { message: 'the key removed' });
    assert.equal((await call('GET', `/instances/${paper}/content`)).items.length, 2);
    assert.deepEqual((await call('GET', `/instances/${paper}/content`)).sources, ['modrinth', 'hangar']);
    assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
  } finally {
    await cleanUp();
    await page.goto(`${BASE}/servers/${serverId}`);
    await page.waitFor(() => document.querySelector('.c-tabs__tab'), { message: 'back on the first server' });
  }
}));

test('the backups and schedules tabs fit a phone screen', { skip, timeout: 60000 }, step('phone', async () => {
  const fits = () => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
  const schedule = await page.evaluate(async (id) => (await (await fetch(`/api/v1/instances/${id}/schedules`, {
    method: 'POST',
    headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' },
    body: JSON.stringify({ name: 'Phone check with a rather long name to wrap', enabled: true, trigger: { kind: 'interval', every_secs: 3600 }, actions: [{ kind: 'restart', warn_secs: [600, 300, 60, 10] }, { kind: 'backup', scope: 'world' }] }),
  })).json()).id, serverId);
  await page.setViewport(375, 812, { mobile: true });
  try {
    await page.click('.c-tabs__tab', 'Schedules');
    await page.waitFor(() => document.querySelector('.c-schedule'), { message: 'a schedule item' });
    assert.ok(await fits(), 'schedules: no sideways scrolling');
    await shot('18-phone-schedules');
    await page.fill('.c-schedules__toolbar select', 'daily-restart');
    await page.waitFor(() => document.querySelector('.c-schedule-editor'), { message: 'the editor' });
    assert.ok(await fits(), 'editor: no sideways scrolling');
    await shot('19-phone-schedule-editor');
    await page.click('.c-schedule-editor button', 'Cancel');
    await page.click('.c-tabs__tab', 'Mods');
    await page.waitFor(() => document.querySelectorAll('.c-content-result').length > 0, { timeout: 30000, message: 'Modrinth results' });
    assert.ok(await fits(), 'mods: no sideways scrolling');
    await shot('20-phone-mods');
    await page.click('.c-tabs__tab', 'Backups');
    await page.waitFor(() => document.querySelector('.c-backups__summary')?.textContent, { message: 'the backups tab' });
    assert.ok(await fits(), 'backups: no sideways scrolling');
    await shot('20-phone-backups-tab');
  } finally {
    await page.setViewport(1280, 900);
    await page.evaluate(async ({ id, schedule }) => fetch(`/api/v1/instances/${id}/schedules/${schedule}`, { method: 'DELETE', headers: { 'X-CSRF': '1' } }), { id: serverId, schedule });
  }
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('stops the server from the UI', { skip, timeout: 120000 }, step('stop', async () => {
  await page.click('.c-actions button', 'Stop');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Stopped', {
    timeout: 100000,
    interval: 1000,
    message: 'the server to stop',
  });
  await shot('07-stopped');
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

/** Opens a server page from the dashboard by name. */
async function openServer(name) {
  await page.goto(`${BASE}/`);
  await page.waitFor((n) => [...document.querySelectorAll('.c-server-card')].some((c) => c.textContent.includes(n)), { args: [name], message: `a card for ${name}` });
  await page.click('.c-server-card', name);
  await page.waitFor((n) => document.querySelector('.l-page-header__title')?.textContent === n, { args: [name], message: `the page of ${name}` });
}

/** Deletes the open server through the UI (typing its name to confirm; `more` answers the
 * question about its backups, if it has any). */
async function deleteOpenServer(name, ...more) {
  page.answerDialogs(name, ...more);
  await page.click('.c-actions button', 'Delete');
  await page.waitFor((n) => location.pathname === '/' && ![...document.querySelectorAll('.c-server-card')].some((c) => c.textContent.includes(n)), {
    args: [name],
    timeout: 20000,
    message: `${name} to disappear from the dashboard`,
  });
}

test('clones a server without its world', { skip, timeout: 180000 }, step('clone', async () => {
  await openServer(SERVER);
  page.answerDialogs('E2E clone', false); // name; "copy the worlds?" → no
  await page.click('.c-actions button', 'Clone');
  await page.waitFor(() => document.querySelector('.l-page-header__title')?.textContent === 'E2E clone', { message: 'the clone page' });
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Stopped', { timeout: 150000, interval: 1000, message: 'the copy to finish' });
  const files = await page.evaluate(async () => {
    const id = location.pathname.split('/').pop();
    return (await (await fetch(`/api/v1/instances/${id}/files`)).json()).entries.map((e) => e.name);
  });
  assert.ok(files.includes('libraries') && files.includes('server.properties'), files.join());
  assert.ok(!files.includes('world') && !files.includes('logs'), `worlds and logs are left out: ${files.join()}`);
  await shot('11-clone');
  // With a backup, deleting asks about it too (answered "delete them": the server's trash).
  await page.evaluate(async () => {
    const id = location.pathname.split('/').pop();
    const started = await (await fetch(`/api/v1/instances/${id}/backups`, {
      method: 'POST', headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' }, body: JSON.stringify({ scope: 'world' }),
    })).json();
    for (let i = 0; i < 100; i += 1) {
      const job = await (await fetch(`/api/v1/jobs/${started.job_id}`)).json();
      if (job.status !== 'running') return;
      await new Promise((resolve) => { setTimeout(resolve, 200); });
    }
  });
  page.answerDialogs('E2E clone', true);
  await page.click('.c-actions button', 'Delete');
  await page.waitFor(() => location.pathname === '/' && ![...document.querySelectorAll('.c-server-card')].some((c) => c.textContent.includes('E2E clone')), {
    timeout: 20000,
    message: 'the clone to disappear from the dashboard',
  });
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs (the backups question was expected)');
}));

test('imports a server folder, runs it, and changes its version', { skip, timeout: 600000 }, step('import', async () => {
  await page.goto(`${BASE}/servers/new`);
  await page.click('a.c-button', 'Import an existing server');
  await page.waitFor(() => document.querySelector('form[data-folder="e2e-import"]'), { message: 'the e2e-import folder' });
  const detected = await page.evaluate(() => {
    const form = document.querySelector('form[data-folder="e2e-import"]');
    return { loader: form.elements.loader.value, version: form.elements.mc_version.value, jar: form.elements.jar.value };
  });
  assert.deepEqual(detected, { loader: 'vanilla', version: '26.3', jar: 'server.jar' }, 'detected from the real jar');
  await shot('12-import');
  await page.fill('form[data-folder="e2e-import"] input[name="name"]', 'E2E import');
  await page.evaluate(() => document.querySelector('form[data-folder="e2e-import"] input[name="eula_accepted"]').click());
  await page.click('form[data-folder="e2e-import"] button[type="submit"]');
  await page.waitFor(() => document.querySelector('.l-page-header__title')?.textContent === 'E2E import', { message: 'the imported server page' });
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Stopped', { timeout: 60000, interval: 500, message: 'the import to finish' });

  await page.click('.c-actions button', 'Start');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Running', { timeout: 150000, interval: 1000, message: 'the imported server to run' });
  await page.click('.c-actions button', 'Stop');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Stopped', { timeout: 100000, interval: 1000, message: 'the imported server to stop' });

  // Change its version: down to 26.2 (a fresh world, since 26.3 worlds don't load in 26.2).
  const id = await page.evaluate(() => location.pathname.split('/').pop());
  await page.evaluate(async (instance) => fetch(`/api/v1/instances/${instance}/files/delete`, {
    method: 'POST', headers: { 'X-CSRF': '1', 'Content-Type': 'application/json' }, body: JSON.stringify({ paths: ['world'] }),
  }), id);
  await page.click('.c-tabs__tab', 'Settings');
  await page.waitFor(() => document.querySelector('.c-version-change [name="target_mc"] option'), { message: 'the version section' });
  assert.match(await page.evaluate(() => document.querySelector('.c-version-change').textContent), /runs Vanilla 26\.3/);
  await page.fill('.c-version-change [name="target_mc"]', '26.2');
  await shot('21-version-change');
  page.answerDialogs('E2E import'); // a downgrade: type the name
  await page.click('.c-version-change button', 'Change version');
  await page.waitFor(() => document.querySelector('.c-instance-heading__meta .c-badge')?.textContent === 'Vanilla 26.2', {
    timeout: 180000, interval: 1000, message: 'the server to run 26.2',
  });
  const backups = await page.evaluate(async (instance) => (await (await fetch(`/api/v1/instances/${instance}/backups`)).json()).backups, id);
  assert.equal(backups[0].kind, 'pre_version_change', 'a safety backup was taken first');
  await page.click('.c-actions button', 'Start');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Running', { timeout: 150000, interval: 1000, message: 'the downgraded server to run' });
  const consoleText = await page.evaluate(async (instance) => (await (await fetch(`/api/v1/instances/${instance}/console`)).json()).console.map((l) => l.text).join('\n'), id);
  assert.match(consoleText, /Starting minecraft server version 26\.2/, 'the real 26.2 server runs');
  await page.click('.c-actions button', 'Stop');
  await page.waitFor(() => document.querySelector('.c-pill')?.textContent === 'Stopped', { timeout: 100000, interval: 1000, message: 'it to stop' });
  await deleteOpenServer('E2E import', true);
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));

test('modpacks: upload a real .mrpack, confirm, and it becomes a server', { skip, timeout: 900000 }, step('modpack', async (t) => {
  const pack = await smallModpack('26.3', 'fabric');
  if (!pack) {
    t.skip('Modrinth has no Fabric modpack for 26.3 that runs on a server');
    return;
  }
  const folder = mkdtempSync(join(tmpdir(), 'gsd-e2e-'));
  const file = join(folder, pack.filename);
  writeFileSync(file, await download(pack));
  await page.goto(`${BASE}/servers/import`);
  await page.waitFor(() => document.querySelector('input[name="modpack_file"]'), { message: 'the upload field' });
  await page.setFiles('input[name="modpack_file"]', [file]);
  await page.waitFor(() => document.querySelector('form[data-pack]'), { timeout: 120000, message: 'the pack form' });
  rmSync(folder, { recursive: true });
  assert.match(await page.evaluate(() => document.querySelector('form[data-pack] .c-import__notes').textContent), /Minecraft 26\.3, fabric/);
  await shot('31-modpack-form');

  await page.fill('form[data-pack] input[name="name"]', 'E2E pack');
  await page.click('form[data-pack] button[type="submit"]', 'Create the server');
  await page.waitFor(() => document.querySelector('form[data-pack] [data-form-error]:not([hidden])')?.textContent.includes('EULA'), { message: 'the EULA is required' });
  await page.click('form[data-pack] input[name="eula_accepted"]');
  await page.click('form[data-pack] button[type="submit"]', 'Create the server');
  await page.waitFor(() => document.querySelector('.l-page-header__title')?.textContent === 'E2E pack', { timeout: 30000, message: 'the new server page' });
  const id = await page.evaluate(() => location.pathname.split('/').pop());

  // Its job installs the loader and then the pack; until it ends, the server has no content list.
  await until('the pack to install', async () => (await page.evaluate(async (i) => (await fetch(`/api/v1/instances/${i}/content`)).status, id)) === 200, 600);
  await page.goto(`${BASE}/servers/${id}`);
  await page.waitFor(() => [...document.querySelectorAll('.c-tabs__tab')].some((tab) => tab.textContent === 'Mods'), { message: 'the server page' });
  await page.click('.c-tabs__tab', 'Mods');
  await page.waitFor(() => document.querySelector('.c-content__table tr[data-file]'), { timeout: 30000, message: 'the pack\'s mods listed' });
  await shot('32-modpack-installed');
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
  await deleteOpenServer('E2E pack', true);
}));

test('modpacks: find one on Modrinth, and it comes back ready to create a server', { skip, timeout: 300000 }, step('modpack-search', async (t) => {
  const pack = await smallModpack('26.3', 'fabric');
  if (!pack) {
    t.skip('Modrinth has no Fabric modpack for 26.3 that runs on a server');
    return;
  }
  await page.goto(`${BASE}/servers/new`);
  await page.click('.l-page-header__actions a', 'From a modpack');
  await page.waitFor(() => document.querySelector('.c-content-result'), { timeout: 30000, message: 'the most downloaded packs' });
  await shot('33-modpack-search');

  await page.fill('select[name="pack_mc"]', '26.3');
  await page.fill('input[name="pack_query"]', pack.title);
  await page.waitFor((id) => document.querySelector(`.c-content-result[data-project="${id}"]`), { timeout: 30000, message: 'the pack in the results', args: [pack.id] });
  await page.evaluate((id) => document.querySelector(`.c-content-result[data-project="${id}"] .c-button--ghost`).click(), pack.id);
  await page.waitFor(() => document.querySelector('.c-content-result__versions select'), { timeout: 30000, message: 'its versions' });
  await page.evaluate((id) => {
    const row = document.querySelector(`.c-content-result[data-project="${id}"]`);
    row.querySelector('.c-content-result__versions .c-button--primary').click();
  }, pack.id);
  await page.waitFor(() => document.querySelector('form[data-pack]'), { timeout: 180000, message: 'the pack form after its download' });
  assert.match(await page.evaluate(() => document.querySelector('form[data-pack] .c-import__notes').textContent), /Minecraft/);
  await shot('34-modpack-found');
  assert.deepEqual(page.errors, [], 'no JavaScript errors or unexpected dialogs');
}));

test('an owner adds a member who then sees only the server they were given', { skip, timeout: 90000 }, step('users', async () => {
  const username = 'e2e-member';
  // Leftover from an interrupted run.
  await page.evaluate(async (name) => {
    const users = await (await fetch('/api/v1/users')).json();
    const old = users.find((u) => u.username === name);
    if (old) await fetch(`/api/v1/users/${old.id}`, { method: 'DELETE', headers: { 'X-CSRF': '1' } });
  }, username);

  await page.goto(`${BASE}/`);
  await page.click('.c-nav__link', 'Users');
  await page.waitFor(() => document.querySelector('.c-users__table tbody tr'), { message: 'the user table' });
  await page.fill('#new-username', username);
  await page.click('.c-users__add button[type="submit"]');
  const password = await page.waitFor(() => document.querySelector('.c-users__secret:not([hidden]) .c-users__password')?.textContent, {
    message: 'the temporary password',
  });

  await page.click(`tr[data-user="${username}"] button`, 'Access');
  await page.waitFor(() => document.querySelector('.c-users__access:not([hidden]) .c-users__grant-add'), { message: 'the access panel' });
  await page.evaluate((server) => {
    const [serverSelect, grantSelect] = document.querySelectorAll('.c-users__grant-add select');
    serverSelect.value = [...serverSelect.options].find((o) => o.textContent === server).value;
    grantSelect.value = 'operator';
  }, SERVER);
  await page.click('.c-users__grant-add button', 'Give access');
  await page.waitFor((server) => [...document.querySelectorAll('.c-users__grant span')].some((s) => s.textContent === server), {
    args: [SERVER],
    message: 'the new grant',
  });
  await shot('13-users');

  // The member, in a separate browser: forced to choose a password, then sees one server.
  const other = await Browser.launch();
  try {
    const member = await other.newPage();
    await member.goto(`${BASE}/login.html`);
    await member.waitFor(() => document.querySelector('form[data-form="login"] button[type="submit"]')?.disabled === false);
    await member.fill('#login-username', username);
    await member.fill('#login-password', password);
    await member.click('form[data-form="login"] button[type="submit"]');
    await member.waitFor(() => !document.querySelector('form[data-form="change-password"]')?.hidden, { message: 'the change-password form' });
    await member.waitFor(() => document.querySelector('form[data-form="change-password"] button[type="submit"]')?.disabled === false);
    await member.fill('#change-current', password);
    await member.fill('#change-new', 'a brand new passphrase');
    await member.click('form[data-form="change-password"] button[type="submit"]');
    await member.waitFor(() => location.pathname === '/' && document.querySelectorAll('.c-server-card').length > 0, { message: "the member's dashboard" });
    const cards = await member.evaluate(() => [...document.querySelectorAll('.c-server-card__name')].map((n) => n.textContent));
    assert.deepEqual(cards, [SERVER], 'only the granted server');
    const usersLink = await member.evaluate(() => document.querySelector('[data-nav="/users"]').hidden);
    assert.equal(usersLink, true, 'members have no Users link');
    assert.deepEqual(member.errors, []);
  } finally {
    await other.close();
  }

  page.answerDialogs(true);
  await page.click(`tr[data-user="${username}"] button`, 'Delete');
  await page.waitFor((name) => !document.querySelector(`tr[data-user="${name}"]`), { args: [username], message: 'the member to be deleted' });
  assert.deepEqual(page.errors, [], 'no JavaScript errors');
}));
