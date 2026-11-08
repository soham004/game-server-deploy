// Tests for web/js/core/content-plan.js. Run: node --test 'tests/web/*.test.mjs'

import assert from 'node:assert/strict';
import { test } from 'node:test';

const { describePlan } = await import('../../web/js/core/content-plan.js');

const step = (title, version, extra = {}) => ({ project: { title }, version: { version_number: version }, required_by: null, replaces: null, ...extra });

test('a plan says what comes, why, and what it replaces', () => {
  const plan = {
    steps: [
      step('AppleSkin', '3.0.10', { replaces: 'appleskin-3.0.9.jar' }),
      step('Fabric API', '0.140.0', { required_by: 'AppleSkin' }),
    ],
    already_installed: [],
  };
  assert.equal(describePlan(plan), 'Install:\n• AppleSkin 3.0.10, replacing appleskin-3.0.9.jar\n• Fabric API 0.140.0 (needed by AppleSkin)');
});

test('dependencies already there are mentioned, not reinstalled', () => {
  const plan = { steps: [step('Architectury', '19.0')], already_installed: ['Fabric API'] };
  assert.equal(describePlan(plan), 'Install:\n• Architectury 19.0\n\nAlready installed: Fabric API.');
});

const { updateLabel, describeIdentified } = await import('../../web/js/core/content-plan.js');

test('an update shows the installed and the new version', () => {
  assert.equal(updateLabel({ installed_version: '0.26.1', version: { version_number: '0.26.3' } }), '0.26.1 → 0.26.3');
  assert.equal(updateLabel({ installed_version: null, version: { version_number: '2.0' } }), 'unknown version → 2.0');
});

test('identifying says what was found, what was not, and what is a duplicate', () => {
  assert.equal(
    describeIdentified({ identified: ['Lithium'], still_unknown: ['a.jar', 'b.jar'], duplicates: ['old-lithium.jar'] }),
    "Found on Modrinth: Lithium. 2 files not found on Modrinth; they can't be updated from here. "
      + 'old-lithium.jar is a second copy of something already installed; keep only one.',
  );
  assert.equal(describeIdentified({ identified: [], still_unknown: ['a.jar'], duplicates: [] }),
    "1 file not found on Modrinth; it can't be updated from here.");
  assert.equal(describeIdentified({ identified: [], still_unknown: [], duplicates: [] }), 'Nothing to identify.');
});
