/**
 * Thin fetch wrapper for the JSON API (`/api/v1`).
 *
 * - Adds the `X-CSRF: 1` header to every state-changing request (the server rejects them without it).
 * - Turns error responses into `ApiError`, carrying the server's stable `code`
 *   (e.g. "invalid_credentials"), so callers can branch on `error.code`.
 */
const BASE = '/api/v1';

export class ApiError extends Error {
  /**
   * @param {number} status HTTP status (0 when the server could not be reached)
   * @param {string} code   stable machine-readable error code
   * @param {string} message human-readable message, safe to show
   */
  constructor(status, code, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

async function request(method, path, body) {
  const headers = { Accept: 'application/json' };
  const init = { method, headers, credentials: 'same-origin' };

  if (method !== 'GET' && method !== 'HEAD') {
    headers['X-CSRF'] = '1';
  }
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }

  let response;
  try {
    response = await fetch(BASE + path, init);
  } catch {
    throw new ApiError(0, 'network_error', 'Cannot reach the server. Check your connection.');
  }

  if (response.status === 204) {
    return null;
  }
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    const error = data?.error ?? {};
    throw new ApiError(
      response.status,
      error.code ?? 'http_error',
      error.message ?? `Request failed (HTTP ${response.status}).`,
    );
  }
  return data;
}

export const api = {
  get: (path) => request('GET', path),
  post: (path, body) => request('POST', path, body),
  put: (path, body) => request('PUT', path, body),
  patch: (path, body) => request('PATCH', path, body),
  delete: (path) => request('DELETE', path),
};
