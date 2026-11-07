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

/** "NeoForge 26.3": the label the server sends (it owns the loader names). */
export function softwareLabel(instance) {
  return instance?.software ?? '';
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

/**
 * "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago", then a date.
 * @param {string} iso
 * @param {number} [now] milliseconds since the epoch (for tests)
 */
export function relativeTime(iso, now = Date.now()) {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days === 1) return 'yesterday';
  if (days < 7) return `${days} days ago`;
  return shortDate(iso);
}
