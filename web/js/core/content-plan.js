/** Wording for content install plans and updates (pure; unit-tested in tests/web/content-plan.test.mjs). */

/** The confirmation text for a plan: what comes, why, and what it replaces. */
export function describePlan(plan) {
  const line = (step) => {
    const what = `${step.project.title} ${step.version.version_number}`;
    const why = step.required_by ? ` (needed by ${step.required_by})` : '';
    const replaces = step.replaces ? `, replacing ${step.replaces}` : '';
    return `• ${what}${why}${replaces}`;
  };
  const byHand = manualDownloads(plan.steps);
  const automatic = plan.steps.filter((step) => !step.version.file.manual_url);
  const missing = plan.not_available.map((dependency) => `• ${dependency.name} (needed by ${dependency.required_by})`);
  const parts = [];
  if (automatic.length) parts.push(`Install:\n${automatic.map(line).join('\n')}`);
  if (byHand.length) {
    const names = plan.steps.filter((step) => step.version.file.manual_url).map(line).join('\n');
    parts.push(`Their authors only allow downloading by hand; you can upload them afterwards:\n${names}`);
  }
  if (plan.already_installed.length) parts.push(`Already installed: ${plan.already_installed.join(', ')}.`);
  if (missing.length) parts.push(`Hosted elsewhere, so not installed; add by hand:\n${missing.join('\n')}`);
  return parts.join('\n\n');
}

/** Files of a plan that wait for the user: their authors only allow downloading by hand. */
export function manualDownloads(steps) {
  return steps
    .filter((step) => step.version.file.manual_url)
    .map((step) => ({ title: step.project.title, filename: step.version.file.filename, url: step.version.file.manual_url }));
}

/** What to call a source. */
export function sourceName(source) {
  return { modrinth: 'Modrinth', hangar: 'Hangar', curseforge: 'CurseForge' }[source] ?? source;
}

/** "0.26.1 → 0.26.3" (the installed version may be unknown). */
export function updateLabel(update) {
  const from = update.installed_version ?? 'unknown version';
  return `${from} → ${update.version.version_number}`;
}

/** What identifying hand-added files did, as one sentence or two. */
export function describeIdentified({ identified, still_unknown: unknown, duplicates }) {
  const parts = [];
  if (identified.length) parts.push(`Found: ${identified.join(', ')}.`);
  if (unknown.length) {
    parts.push(`${countLabel(unknown.length, 'file')} not found on any source; ${unknown.length === 1 ? 'it' : 'they'} can't be updated from here.`);
  }
  if (duplicates.length) {
    parts.push(`${duplicates.join(', ')} ${duplicates.length === 1 ? 'is a second copy' : 'are second copies'} of something already installed; keep only one.`);
  }
  return parts.join(' ') || 'Nothing to identify.';
}


/** One project on one source ("modrinth:gvQqBUqZ"), the way installed files and search hits are matched. */
export function projectKey(source, projectId) {
  return `${source}:${projectId}`;
}

/** The projects a content list has installed (turned on or off), as `projectKey`s. Files added by hand have no source and match nothing. */
export function installedProjects(items) {
  return new Set(items.filter((item) => item.source).map((item) => projectKey(item.source.source, item.source.project_id)));
}

/** "1 mod", "3 plugins". */
export function countLabel(count, noun) {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}
