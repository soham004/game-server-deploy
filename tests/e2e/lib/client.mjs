// A small client for the app's JSON API, shared by the end-to-end scripts. No dependencies:
// Node 22+ has fetch.
//
//   const app = new Client('http://127.0.0.1:18739');
//   await app.signIn();                       // first-run setup, or sign in
//   const { status, body } = await app.call('GET', '/instances');
//   const job = await app.waitForJob(body.job_id);

export const USER = process.env.E2E_USER ?? 'admin';
export const PASSWORD = process.env.E2E_PASSWORD ?? 'correct horse battery';

export class Client {
  /** @param {string} origin e.g. http://127.0.0.1:18739 */
  constructor(origin) {
    this.api = `${origin.replace(/\/$/, '')}/api/v1`;
    this.cookie = '';
  }

  /**
   * One request. `body` is sent as JSON, or as is with `{ raw: true }` (uploads).
   * @returns {Promise<{ status: number, body: any, headers: Headers }>}
   */
  async call(method, path, body, { raw = false } = {}) {
    const headers = { 'X-CSRF': '1' };
    if (this.cookie) headers.Cookie = this.cookie;
    if (body !== undefined && !raw) headers['Content-Type'] = 'application/json';
    const response = await fetch(this.api + path, {
      method,
      headers,
      body: body === undefined ? undefined : raw ? body : JSON.stringify(body),
    });
    const setCookie = response.headers.get('set-cookie');
    if (setCookie) this.cookie = setCookie.split(';')[0];
    const text = await response.text();
    let parsed = null;
    try {
      parsed = text ? JSON.parse(text) : null;
    } catch {
      parsed = text;
    }
    return { status: response.status, body: parsed, headers: response.headers };
  }

  /** Creates the owner on a fresh app (with `setupToken` when it asks for one), or signs in. */
  async signIn({ setupToken } = {}) {
    const setup = await this.call('POST', '/setup', { username: USER, password: PASSWORD, setup_token: setupToken });
    if (setup.status === 201) return;
    const login = await this.call('POST', '/auth/login', { username: USER, password: PASSWORD });
    if (login.status !== 200) throw new Error(`could not sign in as ${USER}: ${login.status} ${JSON.stringify(login.body)}`);
  }

  /** Waits until a job has finished; returns it (with its log). */
  waitForJob(id, seconds = 900) {
    return until(`job ${id}`, async () => {
      const { body } = await this.call('GET', `/jobs/${id}`);
      return body.status === 'running' ? null : body;
    }, seconds);
  }

  /** Waits until an instance's state is `status`; returns the instance. */
  waitForState(id, status, seconds = 300) {
    return until(`instance ${id} to be ${status}`, async () => {
      const { body } = await this.call('GET', `/instances/${id}`);
      return body.state.status === status ? body : null;
    }, seconds);
  }

  /** The server's console lines (text only). */
  async console(id) {
    return (await this.call('GET', `/instances/${id}/console`)).body.console.map((line) => line.text);
  }

  /** An instance by name, or undefined. */
  async findInstance(name) {
    return (await this.call('GET', '/instances')).body.find((instance) => instance.name === name);
  }
}

/** Polls `check` until it returns something truthy (which is returned); fails after `seconds`. */
export async function until(what, check, seconds, intervalMs = 500) {
  const deadline = Date.now() + seconds * 1000;
  while (Date.now() < deadline) {
    const value = await check().catch(() => null);
    if (value) return value;
    await new Promise((resolve) => { setTimeout(resolve, intervalMs); });
  }
  throw new Error(`timed out after ${seconds}s waiting for ${what}`);
}
