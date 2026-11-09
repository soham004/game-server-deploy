// Records the Hangar responses the tests replay, trimmed to the fields the code reads. Rerun when
// the API changes, then review the diff:
//
//   node crates/gsd-mods/tests/fixtures/record-hangar.mjs
//
// Needs the internet. Writes to hangar/ next to this script.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = join(dirname(fileURLToPath(import.meta.url)), 'hangar');
const API = 'https://hangar.papermc.io/api/v1';
const HEADERS = { 'User-Agent': 'game-server-deploy-fixtures (github.com/soham004/game-server-deploy)' };
const MC = '26.3';

async function get(path) {
  const response = await fetch(API + path, { headers: HEADERS });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.json();
}

mkdirSync(HERE, { recursive: true });
const save = (name, data) => writeFileSync(join(HERE, name), `${JSON.stringify(data, null, 1)}\n`);

/** A project keeps what the browse list and the project page show. */
const project = (p) => ({
  id: p.id, name: p.name, namespace: p.namespace, stats: { downloads: p.stats.downloads }, category: p.category,
  description: p.description, lastUpdated: p.lastUpdated, createdAt: p.createdAt, avatarUrl: p.avatarUrl,
  supportedPlatforms: { PAPER: (p.supportedPlatforms.PAPER ?? []).slice(-3) },
  // Only the project endpoint sends the page; search results have null.
  ...(p.mainPageContent ? { mainPageContent: p.mainPageContent.slice(0, 300) } : {}),
});

/** A version keeps its identity, channel, Paper dependencies and Paper download. */
const version = (v) => ({
  id: v.id, projectId: v.projectId, name: v.name, createdAt: v.createdAt, channel: { name: v.channel.name },
  pluginDependencies: { PAPER: v.pluginDependencies.PAPER ?? [] },
  platformDependencies: { PAPER: (v.platformDependencies.PAPER ?? []).slice(-3) },
  downloads: { PAPER: v.downloads.PAPER },
});

const search = await get(`/projects?limit=5&offset=0&platform=PAPER&version=${MC}&sort=-downloads`);
save(`search_paper_${MC}.json`, { pagination: search.pagination, result: search.result.map(project) });

// ViaBackwards requires ViaVersion (and has an optional dependency); Chunky needs nothing.
for (const slug of ['Chunky', 'ViaBackwards', 'ViaVersion']) {
  save(`project_${slug}.json`, project(await get(`/projects/${slug}`)));
  const versions = await get(`/projects/${slug}/versions?limit=25&offset=0&platform=PAPER&platformVersion=${MC}`);
  save(`versions_${slug}_${MC}.json`, { pagination: versions.pagination, result: versions.result.map(version) });
}
console.log('recorded Hangar fixtures');
