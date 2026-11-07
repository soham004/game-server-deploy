/**
 * App-wide navigation without importing the router (avoids circular imports).
 * main.js registers the router's navigate function at startup.
 */
let navigateImpl = (url) => window.location.assign(url);

export function setNavigator(fn) {
  navigateImpl = fn;
}

export function navigate(url, options) {
  return navigateImpl(url, options);
}
