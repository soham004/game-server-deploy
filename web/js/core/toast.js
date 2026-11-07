/** Transient notifications in the `[data-toasts]` container. */
import { h } from './dom.js';

/** More than this and the oldest goes: a pile of toasts hides the controls under it. */
const MAX_VISIBLE = 3;

/**
 * @param {string} message
 * @param {{kind?: 'info' | 'success' | 'error', timeout?: number,
 *          action?: {label: string, onClick: () => void} | null}} [options]
 *   `action` adds a button (the toast then stays longer and closes when it is used).
 */
export function toast(message, { kind = 'info', timeout, action = null } = {}) {
  const container = document.querySelector('[data-toasts]');
  if (!container) return;
  const element = h('div', { class: `c-toast c-toast--${kind}`, role: kind === 'error' ? 'alert' : 'status' },
    h('span', { class: 'c-toast__message' }, message),
    action ? h('button', {
      class: 'c-button c-toast__action',
      type: 'button',
      onClick: () => { element.remove(); action.onClick(); },
    }, action.label) : null,
  );
  // The same message again replaces the old one instead of stacking.
  for (const old of [...container.children]) {
    if (old.querySelector('.c-toast__message')?.textContent === message) old.remove();
  }
  container.append(element);
  while (container.childElementCount > MAX_VISIBLE) container.firstElementChild.remove();
  window.setTimeout(() => element.remove(), timeout ?? defaultTimeout(kind, action));
}

function defaultTimeout(kind, action) {
  if (action) return 8000;
  return kind === 'error' ? 6000 : 4000;
}
