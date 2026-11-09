// The CurseForge API key for tests that use the real API: from the environment (CI sets it from a
// repository secret), or else from the `.env` file at the repository root. Only this one variable is
// taken from the file, and nothing here prints it. The file is ignored by git and Docker: the key is
// for testing and must never ship.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parseEnv } from 'node:util';

const DOT_ENV = fileURLToPath(new URL('../../../.env', import.meta.url));

/** The key, or `undefined` when there is none. An empty value counts as none (a fork's CI has that). */
export function curseForgeKey() {
  if (process.env.CURSEFORGE_API_KEY) return process.env.CURSEFORGE_API_KEY;
  try {
    return parseEnv(readFileSync(DOT_ENV, 'utf8')).CURSEFORGE_API_KEY || undefined;
  } catch {
    return undefined;
  }
}
