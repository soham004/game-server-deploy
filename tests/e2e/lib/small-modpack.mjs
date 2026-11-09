// Finds a real, small server-capable modpack on Modrinth to test with, since which packs exist for
// a Minecraft version changes over time.
//
//   const pack = await smallModpack('26.3', 'fabric');   // { id, versionId, title, filename, url, size } or null

const API = 'https://api.modrinth.com/v2';
const HEADERS = { 'User-Agent': 'game-server-deploy e2e tests' };

async function get(path) {
  const response = await fetch(`${API}${path}`, { headers: HEADERS, signal: AbortSignal.timeout(30_000) });
  if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
  return response.json();
}

export async function smallModpack(mcVersion, loader) {
  const facets = JSON.stringify([
    ['project_type:modpack'],
    [`categories:${loader}`],
    [`versions:${mcVersion}`],
    ['server_side:required', 'server_side:optional'],
  ]);
  const { hits } = await get(`/search?facets=${encodeURIComponent(facets)}&index=downloads&limit=15`);
  const candidates = [];
  for (const hit of hits) {
    const query = new URLSearchParams({ loaders: JSON.stringify([loader]), game_versions: JSON.stringify([mcVersion]) });
    const versions = await get(`/project/${hit.project_id}/version?${query}`);
    const file = versions[0]?.files.find((candidate) => candidate.filename.endsWith('.mrpack'));
    if (file) candidates.push({ id: hit.project_id, versionId: versions[0].id, title: hit.title, filename: file.filename, url: file.url, size: file.size });
  }
  candidates.sort((a, b) => a.size - b.size);
  return candidates[0] ?? null;
}

/** The pack's bytes. */
export async function download(pack) {
  const response = await fetch(pack.url, { headers: HEADERS, signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw new Error(`${pack.url}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}
