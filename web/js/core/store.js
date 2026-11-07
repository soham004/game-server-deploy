/**
 * A minimal observable store: get / set / subscribe. No magic, no proxies.
 *
 *   const store = createStore({ count: 0 });
 *   const unsubscribe = store.subscribe((state) => render(state));
 *   store.set({ count: 1 });
 */
export function createStore(initialState) {
  let state = initialState;
  const listeners = new Set();

  return {
    get() {
      return state;
    },
    /** Shallow-merges `patch` (or the result of `patch(state)`) and notifies subscribers. */
    set(patch) {
      const next = typeof patch === 'function' ? patch(state) : patch;
      state = { ...state, ...next };
      for (const listener of listeners) listener(state);
    },
    /** @returns {() => void} unsubscribe */
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
