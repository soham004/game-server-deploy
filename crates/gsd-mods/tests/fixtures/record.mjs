// Records the Modrinth responses the tests replay, trimmed to the fields the code reads (long
// lists like every game version a project ever supported are cut short). Rerun when the API
// changes, then review the diff:
//
//   node crates/gsd-mods/tests/fixtures/record.mjs
//
// Needs the internet. Writes next to this script.

import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const API = 'https://api.modrinth.com/v2';
const HEADERS = { 'User-Agent': 'game-server-deploy-fixtures (github.com/soham004/game-server-deploy)', 'Content-Type': 'application/json' };
const MC = '26.3';

async function get(path) {
  const response = await fetch(API + path, { headers: HEADERS });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.json();
}

async function post(path, body) {
  const response = await fetch(API + path, { method: 'POST', headers: HEADERS, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.json();
}

const query = (object) => Object.entries(object).map(([k, v]) => `${k}=${encodeURIComponent(typeof v === 'string' ? v : JSON.stringify(v))}`).join('&');
const save = (name, data) => writeFileSync(join(HERE, name), `${JSON.stringify(data, null, 1)}\n`);

/** Search hits keep what the browse list shows. */
const hit = (h) => ({
  project_id: h.project_id, project_type: h.project_type, slug: h.slug, author: h.author, title: h.title,
  description: h.description, categories: h.categories, downloads: h.downloads, icon_url: h.icon_url,
  date_modified: h.date_modified, client_side: h.client_side, server_side: h.server_side,
});

/** A version keeps its identity, compatibility, dependencies and files. */
const version = (v) => ({
  id: v.id, project_id: v.project_id, name: v.name, version_number: v.version_number, version_type: v.version_type,
  date_published: v.date_published, game_versions: v.game_versions.slice(-3), loaders: v.loaders,
  dependencies: v.dependencies, changelog: (v.changelog ?? '').slice(0, 200),
  files: v.files.map((f) => ({ url: f.url, filename: f.filename, primary: f.primary, size: f.size, hashes: f.hashes })),
});

const project = (p) => ({
  id: p.id, slug: p.slug, project_type: p.project_type, title: p.title, description: p.description,
  body: (p.body ?? '').slice(0, 300), client_side: p.client_side, server_side: p.server_side,
  categories: p.categories, loaders: p.loaders, icon_url: p.icon_url, downloads: p.downloads,
  source_url: p.source_url, issues_url: p.issues_url, license: p.license,
});

const facets = (type, loader) => [[`project_type:${type}`], [`categories:${loader}`], [`versions:${MC}`], ['server_side:required', 'server_side:optional']];
const fabric = await get(`/search?${query({ facets: facets('mod', 'fabric'), limit: '5', index: 'downloads' })}`);
save(`search_fabric_${MC}.json`, { ...fabric, hits: fabric.hits.map(hit) });
const paper = await get(`/search?${query({ facets: facets('plugin', 'paper'), limit: '5', index: 'downloads' })}`);
save(`search_paper_${MC}.json`, { ...paper, hits: paper.hits.map(hit) });

// appleskin and architectury-api require fabric-api; lithium needs nothing; chunky has a Paper build.
const cases = [
  ['appleskin', ['fabric']], ['architectury-api', ['fabric']], ['fabric-api', ['fabric']], ['lithium', ['fabric']],
  ['chunky', ['paper', 'spigot', 'bukkit']],
];
const sha1s = [];
for (const [slug, loaders] of cases) {
  save(`project_${slug}.json`, project(await get(`/project/${slug}`)));
  const versions = await get(`/project/${slug}/version?${query({ loaders, game_versions: [MC] })}`);
  save(`versions_${slug}_${loaders[0]}_${MC}.json`, versions.slice(0, 3).map(version));
  sha1s.push(versions[0].files.find((f) => f.primary).hashes.sha1);
}

// Identify by hash (one unknown hash on purpose), and the update check for an older Lithium.
const known = await post('/version_files', { hashes: [...sha1s, '0'.repeat(40)], algorithm: 'sha1' });
save('version_files.json', Object.fromEntries(Object.entries(known).map(([hash, v]) => [hash, version(v)])));
const lithium = await get(`/project/lithium/version?${query({ loaders: ['fabric'] })}`);
const older = lithium.find((v) => v.game_versions.includes(MC) && v.id !== lithium[0].id) ?? lithium[1];
const update = await post('/version_files/update', { hashes: [older.files[0].hashes.sha1], algorithm: 'sha1', loaders: ['fabric'], game_versions: [MC] });
save('version_files_update.json', { request_sha1: older.files[0].hashes.sha1, response: Object.fromEntries(Object.entries(update).map(([hash, v]) => [hash, version(v)])) });
console.log('recorded', cases.length, 'projects');
