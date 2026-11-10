// Bundles entry.mjs into web/vendor/codemirror.js (minified ESM) and writes the licences of every
// package that went into it to web/vendor/codemirror.LICENSES.txt. Run with `npm ci && npm run build`.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { build } from 'esbuild';

const here = dirname(fileURLToPath(import.meta.url));
const vendor = resolve(here, '../../web/vendor');

const result = await build({
  entryPoints: [join(here, 'entry.mjs')],
  bundle: true,
  format: 'esm',
  minify: true,
  target: 'es2022',
  legalComments: 'none',
  metafile: true,
  outfile: join(vendor, 'codemirror.js'),
});

// Every package a bundled file came from, with its version and licence text.
const packages = new Map();
for (const input of Object.keys(result.metafile.inputs)) {
  const match = input.match(/node_modules\/((?:@[^/]+\/)?[^/]+)\//);
  if (match) packages.set(match[1], join(here, 'node_modules', match[1]));
}
const sections = [...packages.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([name, folder]) => {
  const { version, license } = JSON.parse(readFileSync(join(folder, 'package.json'), 'utf8'));
  const file = readdirSync(folder).find((entry) => /^licen[cs]e/i.test(entry));
  const text = file ? readFileSync(join(folder, file), 'utf8').trim() : `(${license})`;
  return `${name} ${version} (${license})\n\n${text}`;
});
writeFileSync(
  join(vendor, 'codemirror.LICENSES.txt'),
  `web/vendor/codemirror.js bundles these packages (tools/codemirror):\n\n${sections.join('\n\n----------------------------------------\n\n')}\n`,
);
const size = result.metafile.outputs[Object.keys(result.metafile.outputs)[0]].bytes;
console.log(`codemirror.js: ${(size / 1024).toFixed(0)} KiB from ${packages.size} packages`);
