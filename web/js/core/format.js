/** Display formatting helpers. */

const STATE_LABELS = {
  stopped: 'Stopped',
  starting: 'Starting',
  running: 'Running',
  stopping: 'Stopping',
  crashed: 'Crashed',
};

/** Human label for an instance, taking install state into account. */
export function stateLabel(instance) {
  if (instance.busy) return 'Installing';
  if (!instance.installed) return 'Not installed';
  return STATE_LABELS[instance.state?.status] ?? 'Unknown';
}

/** A stable key for styling: running, stopped, crashed, busy, … */
export function stateKey(instance) {
  if (instance.busy) return 'busy';
  if (!instance.installed) return 'uninstalled';
  return instance.state?.status ?? 'stopped';
}

const LOADER_NAMES = { vanilla: 'Vanilla', paper: 'Paper', fabric: 'Fabric' };

/** "Paper 26.3" style label from an instance's server settings. */
export function softwareLabel(server) {
  const loader = LOADER_NAMES[server?.loader] ?? server?.loader ?? '';
  return `${loader} ${server?.mc_version ?? ''}`.trim();
}

export function memoryLabel(megabytes) {
  return megabytes >= 1024 ? `${(megabytes / 1024).toFixed(megabytes % 1024 === 0 ? 0 : 1)} GB` : `${megabytes} MB`;
}

/** Short date for version lists, e.g. "15 Sep 2026". */
export function shortDate(iso) {
  if (!iso) return '';
  return new Date(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

/** "12:01:22" from an ISO timestamp. */
export function clockTime(iso) {
  return new Date(iso).toLocaleTimeString(undefined, { hour12: false });
}
