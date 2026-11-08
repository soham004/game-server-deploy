// Hard limits in production conditions: drives an app that cgroup-systemd.sh started as a systemd
// service with `Delegate=yes`, through a real server start, and checks the server landed in its
// own control group with the expected limits and its own user. Run by cgroup-systemd.sh (as root);
// E2E_URL points at the app.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { Client } from './lib/client.mjs';

const URL = process.env.E2E_URL;
const skip = URL ? false : 'run through tests/e2e/cgroup-systemd.sh';
const MIB = 1024 * 1024;

test('a server runs in its own control group with hard limits', { skip, timeout: 15 * 60_000 }, async () => {
  const client = new Client(URL);
  await client.signIn();
  const { body: system } = await client.call('GET', '/system');
  assert.equal(system.hard_limits, true, `hard limits are on under systemd with Delegate=yes: ${system.hard_limits_problem}`);

  const created = await client.call('POST', '/instances', { name: 'Cgroup test', mc_version: '26.3', loader: 'vanilla', max_memory_mb: 1024, eula_accepted: true });
  assert.equal(created.status, 202, JSON.stringify(created.body));
  const id = created.body.instance.id;
  const job = await client.waitForJob(created.body.job_id, 600);
  assert.equal(job.status, 'succeeded', job.error);

  await client.call('POST', `/instances/${id}/start`);
  const running = await client.waitForState(id, 'running', 240);
  const pid = running.state.pid;
  const group = readFileSync(`/proc/${pid}/cgroup`, 'utf8').trim().split('::')[1];
  assert.ok(group.endsWith(`/instances/${id}`), `the server's group: ${group}`);
  const read = (name) => readFileSync(`/sys/fs/cgroup${group}/${name}`, 'utf8').trim();
  // A 1024 MB heap gets 1024 MB more for the JVM itself (D-034).
  assert.equal(read('memory.max'), String(2048 * MIB));
  assert.equal(read('memory.swap.max'), '0');
  assert.equal(read('pids.max'), '2048');
  console.log(`memory.current ${Math.round(Number(read('memory.current')) / MIB)} MiB, pids.current ${read('pids.current')}`);
  const uid = readFileSync(`/proc/${pid}/status`, 'utf8').match(/^Uid:\s+(\d+)/m)[1];
  assert.notEqual(uid, '0', 'the server runs as its own user');

  const { body: report } = await client.call('POST', '/system/isolation-check', {});
  const failed = report.checks.filter((check) => check.enforced && !check.passed);
  assert.deepEqual(failed, [], 'every enforced isolation check passes');
  const memory = report.checks.find((check) => check.name === 'Runs under a hard memory limit');
  assert.ok(memory.passed && memory.enforced, JSON.stringify(memory));

  await client.call('POST', `/instances/${id}/stop`);
  await client.waitForState(id, 'stopped', 120);
});
