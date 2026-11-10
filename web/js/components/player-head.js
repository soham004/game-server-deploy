/**
 * A player's head: their skin's face from the server (which asks Mojang and keeps a copy), or a plain
 * face of our own when there is none (offline-mode players, default skins, Mojang unreachable).
 *
 *   row.append(head(uuid, 'Steve'));
 */
import { h } from '../core/dom.js';

/** `size`: '' (list rows) or 'is-large' (a player's card). */
export function head(uuid, name, size = '') {
  const fallback = () => h('span', { class: `c-player-head is-placeholder ${size}`.trim(), 'aria-hidden': 'true' },
    (name ?? '?').slice(0, 1).toUpperCase());
  if (!uuid) return fallback();
  const image = h('img', {
    class: `c-player-head ${size}`.trim(),
    src: `/api/v1/players/${encodeURIComponent(uuid)}/head.png`,
    alt: '',
    width: 32,
    height: 32,
    loading: 'lazy',
  });
  image.addEventListener('error', () => image.replaceWith(fallback()), { once: true });
  return image;
}
