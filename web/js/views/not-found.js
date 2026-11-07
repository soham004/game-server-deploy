/** Fallback view for unknown client-side routes. */
import { h } from '../core/dom.js';

export function mount(root) {
  root.append(
    h('section', { class: 'c-card c-empty' },
      h('h1', { class: 'c-empty__title' }, 'Page not found'),
      h('p', { class: 'c-empty__text' }, 'This page does not exist.'),
      h('a', { class: 'c-button', href: '/' }, 'Back to servers'),
    ),
  );
}
