/** Wording for content install plans and updates (pure; unit-tested in tests/web/content-plan.test.mjs). */

/** The confirmation text for a plan: what comes, why, and what it replaces. */
export function describePlan(plan) {
  const lines = plan.steps.map((step) => {
    const what = `${step.project.title} ${step.version.version_number}`;
    const why = step.required_by ? ` (needed by ${step.required_by})` : '';
    const replaces = step.replaces ? `, replacing ${step.replaces}` : '';
    return `• ${what}${why}${replaces}`;
  });
  const kept = plan.already_installed.length ? `\n\nAlready installed: ${plan.already_installed.join(', ')}.` : '';
  return `Install:\n${lines.join('\n')}${kept}`;
}

/** "0.26.1 → 0.26.3" (the installed version may be unknown). */
export function updateLabel(update) {
  const from = update.installed_version ?? 'unknown version';
  return `${from} → ${update.version.version_number}`;
}

/** What identifying hand-added files did, as one sentence or two. */
export function describeIdentified({ identified, still_unknown: unknown, duplicates }) {
  const parts = [];
  if (identified.length) parts.push(`Found on Modrinth: ${identified.join(', ')}.`);
  if (unknown.length) {
    parts.push(`${count(unknown.length, 'file')} not found on Modrinth; ${unknown.length === 1 ? 'it' : 'they'} can't be updated from here.`);
  }
  if (duplicates.length) {
    parts.push(`${duplicates.join(', ')} ${duplicates.length === 1 ? 'is a second copy' : 'are second copies'} of something already installed; keep only one.`);
  }
  return parts.join(' ') || 'Nothing to identify.';
}

function count(n, noun) {
  return `${n} ${noun}${n === 1 ? '' : 's'}`;
}
