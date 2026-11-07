/** Transient notifications in the `[data-toasts]` container. */
import { h } from './dom.js';

/**
 * @param {string} message
 * @param {{kind?: 'info' | 'success' | 'error', timeout?: number}} [options]
 */
export function toast(message, { kind = 'info', timeout = 5000 } = {}) {
  const container = document.querySelector('[data-toasts]');
  if (!container) return;
  const element = h('div', { class: `c-toast c-toast--${kind}`, role: kind === 'error' ? 'alert' : 'status' }, message);
  container.append(element);
  window.setTimeout(() => element.remove(), timeout);
}
