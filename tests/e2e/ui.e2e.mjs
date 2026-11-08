// End-to-end UI test in a real (headless) browser against a running app with a real server.
// Set up with loaders.e2e.mjs, serve with serve.sh (see README.md), then:
//
//   E2E_URL=http://localhost:18739 E2E_SCREENSHOTS=/tmp/shots node --test tests/e2e/ui.e2e.mjs
//
// E2E_SERVER picks the server to drive (default "E2E neoforge"); E2E_USER and E2E_PASSWORD sign in.
// Skipped unless E2E_URL is set.

import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, test } from 'node:test';

import { Browser } from './cdp.mjs';

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

test('signs in and lists the servers', { skip }, step('sign-in', async () => {
  await page.goto(`${BASE}/`);
  await page.waitFor(() => location.pathname === '/login.html', { message: 'redirect to the login page' });
  // The button is enabled only once the page script handles the form (see core/forms.js).
  await page.waitFor(() => document.querySelector('form[data-form="login"] button[type="submit"]')?.disabled === false, {
    message: 'the sign-in form to be ready',
  });
  await page.fill('#login-username', process.env.E2E_USER ?? 'admin');
  await page.fill('#login-password', process.env.E2E_PASSWORD ?? 'correct horse battery');
  await page.click('form[data-form="login"] button[type="submit"]');
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
    await page.click('.c-tabs__tab', 'Backups');
    await page.waitFor(() => document.querySelector('.c-backups__summary')?.textContent, { message: 'the backups tab' });
    assert.ok(await fits(), 'backups: no sideways scrolling');
    await shot('20-phone-backups');
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
