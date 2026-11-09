// A stand-in for the CurseForge API, for the browser suite: it has no real key to use, and a key
// must never be handled by these tests. It follows the documented schema, like the Rust tests' mock
// (gsd_mods::test_support), with two plugins: one that downloads, and one whose author only allows
// downloading by hand. `curseforge.e2e.mjs` checks the real API, with a key the user supplies.
//
//   node tests/e2e/lib/mock-curseforge.mjs 18740

import { createHash } from 'node:crypto';
import { createServer } from 'node:http';

/** The only key this accepts. */
export const MOCK_KEY = 'e2e-curseforge-key';

const port = Number(process.argv[2] ?? 18740);
const origin = `http://127.0.0.1:${port}`;

const jar = Buffer.from('not a real jar, but a real download');
const page = (slug) => `https://www.curseforge.com/minecraft/bukkit-plugins/${slug}`;

const project = (id, name, slug) => ({
  id, gameId: 432, name, slug, classId: 5, summary: `${name} from the stand-in`, downloadCount: 42,
  links: { websiteUrl: page(slug) }, authors: [{ id: 1, name: 'e2e', url: 'https://example.test' }],
  categories: [{ id: 1, name: 'Utility' }], logo: null, dateModified: '2026-11-01T00:00:00Z', allowModDistribution: true,
});

const file = (id, modId, fileName, downloadUrl) => ({
  id, modId, displayName: fileName.replace('.jar', ''), fileName, releaseType: 1, fileDate: '2026-11-01T00:00:00Z',
  fileLength: jar.length, downloadUrl, gameVersions: ['26.3'], dependencies: [], isAvailable: true, isServerPack: false,
  hashes: [{ value: createHash('sha1').update(jar).digest('hex'), algo: 1 }], fileFingerprint: 1,
});

const projects = [project(9001, 'E2E CF Plugin', 'e2e-cf-plugin'), project(9002, 'E2E CF Locked', 'e2e-cf-locked')];
const files = {
  9001: [file(9101, 9001, 'e2e-cf-plugin.jar', `${origin}/download/e2e-cf-plugin.jar`)],
  9002: [file(9102, 9002, 'e2e-cf-locked.jar', null)],
};

const json = (response, status, body) => {
  response.writeHead(status, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify(body));
};

async function bodyOf(request) {
  let text = '';
  for await (const chunk of request) text += chunk;
  return text ? JSON.parse(text) : {};
}

const server = createServer(async (request, response) => {
  const { pathname, searchParams } = new URL(request.url, origin);
  if (pathname.startsWith('/download/')) {
    response.writeHead(200);
    response.end(jar);
    return;
  }
  if (request.headers['x-api-key'] !== MOCK_KEY) {
    json(response, 403, {});
    return;
  }
  const filesOf = pathname.match(/^\/mods\/(\d+)\/files$/);
  const one = pathname.match(/^\/mods\/(\d+)$/);
  if (pathname === '/games/432') {
    json(response, 200, { data: { id: 432 } });
  } else if (pathname === '/mods/search') {
    const hits = searchParams.get('classId') === '5' ? projects : [];
    json(response, 200, { data: hits, pagination: { index: 0, pageSize: 20, resultCount: hits.length, totalCount: hits.length } });
  } else if (filesOf) {
    json(response, 200, { data: files[filesOf[1]] ?? [] });
  } else if (one) {
    const found = projects.find((candidate) => candidate.id === Number(one[1]));
    json(response, found ? 200 : 404, found ? { data: found } : {});
  } else if (pathname === '/mods') {
    const { modIds } = await bodyOf(request);
    json(response, 200, { data: projects.filter((candidate) => modIds.includes(candidate.id)) });
  } else if (pathname === '/fingerprints/432') {
    // Whatever is asked about first is the locked plugin: the suite only ever uploads that one.
    const { fingerprints } = await bodyOf(request);
    const matched = fingerprints.slice(0, 1).map((fingerprint) => ({ id: 9002, file: { ...files[9002][0], fileFingerprint: fingerprint } }));
    json(response, 200, { data: { exactMatches: matched } });
  } else {
    json(response, 404, {});
  }
});

server.listen(port, '127.0.0.1');
