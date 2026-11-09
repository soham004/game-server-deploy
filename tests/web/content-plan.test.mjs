// Tests for web/js/core/content-plan.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { describePlan } = await import('../../web/js/core/content-plan.js');

const step = (title, version, extra = {}, file = {}) => ({ project: { title }, version: { version_number: version, file }, required_by: null, replaces: null, ...extra });

test('a plan says what comes, why, and what it replaces', () => {
  const plan = {
    steps: [
      step('AppleSkin', '3.0.10', { replaces: 'appleskin-3.0.9.jar' }),
      step('Fabric API', '0.140.0', { required_by: 'AppleSkin' }),
    ],
    already_installed: [],
    not_available: [],
  };
  assert.equal(describePlan(plan), 'Install:\n• AppleSkin 3.0.10, replacing appleskin-3.0.9.jar\n• Fabric API 0.140.0 (needed by AppleSkin)');
});

test('dependencies already there are mentioned, not reinstalled', () => {
  const plan = { steps: [step('Architectury', '19.0')], already_installed: ['Fabric API'], not_available: [] };
  assert.equal(describePlan(plan), 'Install:\n• Architectury 19.0\n\nAlready installed: Fabric API.');
});

test('dependencies hosted elsewhere are named, since they are not coming', () => {
  const plan = { steps: [step('Shop', '1.0')], already_installed: [], not_available: [{ name: 'Vault', required_by: 'Shop' }] };
  assert.equal(describePlan(plan), 'Install:\n• Shop 1.0\n\nHosted elsewhere, so not installed; add by hand:\n• Vault (needed by Shop)');
});

const { manualDownloads, sourceName } = await import('../../web/js/core/content-plan.js');

const byHand = { filename: 'locked.jar', manual_url: 'https://www.curseforge.com/minecraft/mc-mods/locked/files/7' };

test('files only their authors can hand out are named apart, not promised', () => {
  const both = { steps: [step('Open', '1.0'), step('Locked', '2.0', {}, byHand)], already_installed: [], not_available: [] };
  assert.equal(describePlan(both), 'Install:\n• Open 1.0\n\nTheir authors only allow downloading by hand; you can upload them afterwards:\n• Locked 2.0');
  const only = { steps: [step('Locked', '2.0', {}, byHand)], already_installed: [], not_available: [] };
  assert.equal(describePlan(only), 'Their authors only allow downloading by hand; you can upload them afterwards:\n• Locked 2.0');
  assert.deepEqual(manualDownloads(both.steps), [{ title: 'Locked', filename: 'locked.jar', url: byHand.manual_url }]);
});

test('sources have names to show', () => {
  assert.equal(sourceName('modrinth'), 'Modrinth');
  assert.equal(sourceName('hangar'), 'Hangar');
  assert.equal(sourceName('curseforge'), 'CurseForge');
  assert.equal(sourceName('new_one'), 'new_one', 'an unknown source still says something');
});

const { updateLabel, describeIdentified } = await import('../../web/js/core/content-plan.js');

test('an update shows the installed and the new version', () => {
  assert.equal(updateLabel({ installed_version: '0.26.1', version: { version_number: '0.26.3' } }), '0.26.1 → 0.26.3');
  assert.equal(updateLabel({ installed_version: null, version: { version_number: '2.0' } }), 'unknown version → 2.0');
});

test('identifying says what was found, what was not, and what is a duplicate', () => {
  assert.equal(
    describeIdentified({ identified: ['Lithium'], still_unknown: ['a.jar', 'b.jar'], duplicates: ['old-lithium.jar'] }),
    "Found: Lithium. 2 files not found on any source; they can't be updated from here. "
      + 'old-lithium.jar is a second copy of something already installed; keep only one.',
  );
  assert.equal(describeIdentified({ identified: [], still_unknown: ['a.jar'], duplicates: [] }),
    "1 file not found on any source; it can't be updated from here.");
  assert.equal(describeIdentified({ identified: [], still_unknown: [], duplicates: [] }), 'Nothing to identify.');
});

const { installedProjects, projectKey } = await import('../../web/js/core/content-plan.js');

test('installed projects are known by source and id, turned off or not; hand-added files match nothing', () => {
  const items = [
    { file: 'lithium.jar', enabled: true, source: { source: 'modrinth', project_id: 'gvQqBUqZ', version_id: 'a' } },
    { file: 'chunky.jar', enabled: false, source: { source: 'modrinth', project_id: 'fALzjamp', version_id: 'b' } },
    { file: 'handmade.jar', enabled: true, source: null },
  ];
  const installed = installedProjects(items);
  assert.deepEqual([...installed].sort(), ['modrinth:fALzjamp', 'modrinth:gvQqBUqZ']);
  assert.ok(installed.has(projectKey('modrinth', 'gvQqBUqZ')));
  assert.ok(!installed.has(projectKey('hangar', 'gvQqBUqZ')), 'the same id on another source is another project');
});

const { countLabel } = await import('../../web/js/core/content-plan.js');

test('counts read naturally', () => {
  assert.equal(countLabel(1, 'mod'), '1 mod');
  assert.equal(countLabel(3, 'plugin'), '3 plugins');
  assert.equal(countLabel(0, 'mod'), '0 mods');
});
