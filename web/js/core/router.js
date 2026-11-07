/**
 * Minimal History-API router.
 *
 * Each route maps a path pattern (with `:params`) to a lazily imported view module exporting
 * `mount(root, params)`, which renders into `root` and may return an `unmount()` function.
 * Unmount must release everything the view subscribed to (store, WebSocket topics, timers).
 */
import { toast } from './toast.js';

/**
 * @param {{
 *   root: HTMLElement,
 *   routes: {path: string, view: () => Promise<any>}[],
 *   notFound: () => Promise<any>,
 *   onNavigate?: (pathname: string) => void,
 * }} options
 */
export function createRouter({ root, routes, notFound, onNavigate }) {
  let unmountCurrent = null;
  let renderToken = 0;

  async function render() {
    const token = ++renderToken;
    const pathname = window.location.pathname;
    const matched = matchRoute(routes, pathname);

    try {
      const view = await (matched ? matched.route.view() : notFound());
      if (token !== renderToken) return; // a newer navigation won the race

      unmountCurrent?.();
      unmountCurrent = null;
      root.replaceChildren();
      unmountCurrent = (await view.mount(root, matched?.params ?? {})) ?? null;
      onNavigate?.(pathname);
      root.focus({ preventScroll: true });
    } catch (error) {
      console.error(error);
      toast(`Could not open this page: ${error.message}`, { kind: 'error' });
    }
  }

  function navigate(url, { replace = false } = {}) {
    window.history[replace ? 'replaceState' : 'pushState'](null, '', url);
    return render();
  }

  /** Turns same-origin link clicks into client-side navigation. */
  function interceptLinks(event) {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest('a[href]');
    if (!link || link.target || link.hasAttribute('download')) return;

    const url = new URL(link.href);
    if (url.origin !== window.location.origin || url.pathname.endsWith('.html')) return;
    event.preventDefault();
    navigate(url.pathname + url.search + url.hash);
  }

  return {
    start() {
      document.addEventListener('click', interceptLinks);
      window.addEventListener('popstate', render);
      return render();
    },
    navigate,
  };
}

function matchRoute(routes, pathname) {
  for (const route of routes) {
    const params = matchPattern(route.path, pathname);
    if (params) return { route, params };
  }
  return null;
}

function matchPattern(pattern, pathname) {
  const expected = pattern.split('/').filter(Boolean);
  const actual = pathname.split('/').filter(Boolean);
  if (expected.length !== actual.length) return null;

  const params = {};
  for (let i = 0; i < expected.length; i += 1) {
    if (expected[i].startsWith(':')) {
      params[expected[i].slice(1)] = decodeURIComponent(actual[i]);
    } else if (expected[i] !== actual[i]) {
      return null;
    }
  }
  return params;
}
