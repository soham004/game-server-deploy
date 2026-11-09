/**
 * Runs `fn` once calls have stopped for `wait` ms (with the last call's arguments). Unit-tested
 * in tests/web/debounce.test.mjs.
 *
 *   const later = debounce(search, 200);
 *   input.addEventListener('input', () => later());
 *   later.cancel();   // drop a pending call (e.g. when searching right away instead)
 */
export function debounce(fn, wait) {
  let timer = null;
  function debounced(...args) {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      fn(...args);
    }, wait);
  }
  debounced.cancel = () => {
    clearTimeout(timer);
    timer = null;
  };
  return debounced;
}
