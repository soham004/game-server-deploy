/**
 * What the players tab works out without the page: ban lengths, play time, how changes are made, and
 * which players match a search. Kept apart from the DOM so it can be tested on its own.
 */

/** Ban lengths offered, in milliseconds; `forever` has none. */
export const BAN_LENGTHS = [
  ['forever', 'Forever', null],
  ['hour', '1 hour', 60 * 60 * 1000],
  ['day', '1 day', 24 * 60 * 60 * 1000],
  ['week', '7 days', 7 * 24 * 60 * 60 * 1000],
  ['month', '30 days', 30 * 24 * 60 * 60 * 1000],
];

/**
 * When a ban of this length, starting now, ends: an ISO time, or null for one that doesn't.
 * @param {string} key one of BAN_LENGTHS
 * @param {number} [now] milliseconds since the epoch
 */
export function banExpiry(key, now = Date.now()) {
  const length = BAN_LENGTHS.find(([id]) => id === key)?.[2] ?? null;
  return length === null ? null : new Date(now + length).toISOString();
}

/** "45 s", "12 min", "3 h 5 min", "2 days 4 h": how long someone played. */
export function playedLabel(seconds) {
  if (seconds === null || seconds === undefined) return '';
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return `${days} ${days === 1 ? 'day' : 'days'}${rest ? ` ${rest} h` : ''}`;
}

/** Seconds since an ISO time (0 for one in the future). */
export function secondsSince(iso, now = Date.now()) {
  return Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
}

/** One sentence on how a change is made right now. */
export function channelNote(channel) {
  switch (channel) {
    case 'management':
      return 'Changes go to the running server through its management protocol.';
    case 'console':
      return 'Changes go to the running server as console commands. Choosing an operator level and bans that end need Minecraft 1.21.9 or later, or a stopped server.';
    case 'files':
      return 'The server is stopped: changes go into its files and apply when it starts.';
    default:
      return 'Wait until the server has started or stopped to make changes.';
  }
}

/** Whether a player's name or UUID contains the search (any case). */
export function playerMatches(player, query) {
  const wanted = query.trim().toLowerCase();
  if (!wanted) return true;
  return player.name.toLowerCase().includes(wanted) || (player.uuid ?? '').toLowerCase().includes(wanted);
}

/** A name an account could have: letters, digits and _, at most 16 (what the server accepts). */
export function isPlayerName(name) {
  return /^[A-Za-z0-9_]{1,16}$/.test(name);
}

/** What the lists say about one player, for its card. */
export function standing(view, name) {
  const same = (other) => (other ?? '').toLowerCase() === name.toLowerCase();
  return {
    online: view.online.find((player) => same(player.name)) ?? null,
    operator: view.operators.find((op) => same(op.name)) ?? null,
    allowed: view.allowlist.players.some((player) => same(player.name)),
    ban: view.bans.find((ban) => same(ban.name)) ?? null,
  };
}
