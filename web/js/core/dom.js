/**
 * Tiny DOM helpers.
 *
 * `h()` builds elements from plain values. Text is always inserted as text nodes, never parsed
 * as HTML, so server-provided strings are safe to pass in.
 *
 *   h('button', { class: 'c-button', type: 'button', onClick: save }, 'Save')
 */

/**
 * @param {string} tag
 * @param {Record<string, unknown> | null} [props] attributes; `class`, `dataset`, and `onEvent` handlers are special
 * @param {...unknown} children nodes, strings, numbers, arrays; null/false are skipped
 * @returns {HTMLElement}
 */
export function h(tag, props = null, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === 'class') {
      element.className = String(value);
    } else if (key === 'dataset') {
      Object.assign(element.dataset, value);
    } else if (key.startsWith('on') && typeof value === 'function') {
      element.addEventListener(key.slice(2).toLowerCase(), value);
    } else {
      element.setAttribute(key, value === true ? '' : String(value));
    }
  }
  appendChildren(element, children);
  return element;
}

function appendChildren(parent, children) {
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    parent.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

/** First element matching `selector` (throws if missing, so typos fail loudly). */
export function $(selector, root = document) {
  const element = root.querySelector(selector);
  if (!element) throw new Error(`Element not found: ${selector}`);
  return element;
}
