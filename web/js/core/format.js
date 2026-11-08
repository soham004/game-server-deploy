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

/** "512 B", "3.4 KB", "120 MB", "1.5 GB": a file or archive size. */
export function sizeLabel(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** "8 Nov 2026, 03:14": a moment people pick from a list (backups). */
export function dateTime(iso) {
  if (!iso) return '';
  return new Date(iso).toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false });
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

/**
 * "in under a minute", "in 5 min", "in 3 h", then "on <date>": when something will happen.
 * @param {string} iso
 * @param {number} [now] milliseconds since the epoch (for tests)
 */
export function untilTime(iso, now = Date.now()) {
  const seconds = Math.round((new Date(iso).getTime() - now) / 1000);
  if (Number.isNaN(seconds)) return '';
  if (seconds < 60) return 'in under a minute';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `in ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `in ${hours} h`;
  return `on ${dateTime(iso)}`;
}
