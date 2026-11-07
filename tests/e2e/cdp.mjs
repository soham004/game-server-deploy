// A minimal Chrome DevTools Protocol client for end-to-end UI tests, with no dependencies
// (Node 22+ has a global WebSocket). Drives a headless Chrome or Edge: open pages, run
// JavaScript in them, wait for conditions, take screenshots, and collect page errors.

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

/** Chrome or Edge, from `$BROWSER` or the usual install locations. */
export function findBrowser() {
  const candidates = [
    process.env.BROWSER,
    'C:/Program Files/Google/Chrome/Application/chrome.exe',
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ].filter(Boolean);
  const found = candidates.find((path) => existsSync(path));
  if (found) return found;
  throw new Error('No Chrome/Edge found; set BROWSER to its executable.');
}

export class Browser {
  static async launch({ executable = findBrowser(), width = 1280, height = 900 } = {}) {
    const profile = mkdtempSync(join(tmpdir(), 'gsd-e2e-'));
    const child = spawn(executable, [
      '--headless=new',
      '--remote-debugging-port=0',
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-extensions',
      `--window-size=${width},${height}`,
      'about:blank',
    ], { stdio: 'ignore' });
    const endpoint = await readEndpoint(profile);
    const socket = new WebSocket(endpoint);
    await new Promise((resolve, reject) => {
      socket.addEventListener('open', resolve, { once: true });
      socket.addEventListener('error', reject, { once: true });
    });
    return new Browser(child, profile, socket, { width, height });
  }

  constructor(child, profile, socket, viewport) {
    this.child = child;
    this.profile = profile;
    this.socket = socket;
    this.viewport = viewport;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Set();
    socket.addEventListener('message', (event) => this.#dispatch(JSON.parse(event.data)));
  }

  /** Sends a CDP command (optionally to a page session) and resolves with its result. */
  send(method, params = {}, sessionId = undefined) {
    const id = this.nextId++;
    this.socket.send(JSON.stringify({ id, method, params, sessionId }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject, method }));
  }

  async newPage() {
    const { targetId } = await this.send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await this.send('Target.attachToTarget', { targetId, flatten: true });
    const page = new Page(this, sessionId);
    await page.init();
    return page;
  }

  async close() {
    try {
      await this.send('Browser.close');
    } catch {
      // already gone
    }
    this.socket.close();
    await Promise.race([new Promise((resolve) => this.child.once('exit', resolve)), sleep(5000)]);
    this.child.kill();
    for (let attempt = 0; attempt < 10; attempt += 1) {
      try {
        rmSync(this.profile, { recursive: true, force: true });
        return;
      } catch {
        await sleep(300); // Windows keeps files locked briefly after exit
      }
    }
  }

  #dispatch(message) {
    if (message.id !== undefined) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (!pending) return;
      if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
      else pending.resolve(message.result);
      return;
    }
    for (const listener of this.listeners) listener(message);
  }
}

export class Page {
  constructor(browser, sessionId) {
    this.browser = browser;
    this.sessionId = sessionId;
    /** Uncaught exceptions and console.error calls in the page. */
    this.errors = [];
    /** Answers for upcoming alert/confirm/prompt dialogs (see `answerDialogs`). */
    this.dialogAnswers = [];
    browser.listeners.add((message) => {
      if (message.sessionId !== sessionId) return;
      if (message.method === 'Page.javascriptDialogOpening') {
        this.#answerDialog(message.params);
      } else if (message.method === 'Runtime.exceptionThrown') {
        this.errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        this.errors.push(message.params.args.map((arg) => arg.value ?? arg.description).join(' '));
      }
    });
  }

  send(method, params = {}) {
    return this.browser.send(method, params, this.sessionId);
  }

  async init() {
    await this.send('Page.enable');
    await this.send('Runtime.enable');
    const { width, height } = this.browser.viewport;
    await this.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  }

  async goto(url) {
    await this.send('Page.navigate', { url });
    await this.waitFor(() => document.readyState === 'complete');
  }

  /** Runs `fn` (serialised, so it can't close over test variables) with `args` in the page. */
  async evaluate(fn, ...args) {
    const expression = `(${fn.toString()})(...${JSON.stringify(args)})`;
    const { result, exceptionDetails } = await this.send('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (exceptionDetails) {
      throw new Error(`evaluate failed: ${exceptionDetails.exception?.description ?? exceptionDetails.text}`);
    }
    return result.value;
  }

  /** Polls `fn(...args)` in the page until it returns something truthy. */
  async waitFor(fn, { timeout = 15000, interval = 100, args = [], message } = {}) {
    const deadline = Date.now() + timeout;
    let last;
    while (Date.now() < deadline) {
      try {
        last = await this.evaluate(fn, ...args);
        if (last) return last;
      } catch (error) {
        last = error.message; // navigation in progress
      }
      await sleep(interval);
    }
    throw new Error(`Timed out after ${timeout} ms waiting for ${message ?? fn.toString()} (last: ${JSON.stringify(last)})`);
  }

  /**
   * Clicks the first element matching `selector` whose text contains `text` (if given),
   * waiting up to `timeout` ms for it to appear (pages render after navigation).
   */
  async click(selector, text = null, { timeout = 10000 } = {}) {
    const clicked = await this.waitFor((sel, label) => {
      const element = [...document.querySelectorAll(sel)]
        .find((el) => label === null || el.textContent.includes(label));
      if (!element) return false;
      element.scrollIntoView({ block: 'center' });
      element.click();
      return true;
    }, { args: [selector, text], timeout, message: `${selector} ${text ?? ''}`.trim() });
    return clicked;
  }

  /** Sets a form control's value the way typing would, firing input and change events. */
  fill(selector, value) {
    return this.evaluate((sel, text) => {
      const element = document.querySelector(sel);
      if (!element) throw new Error(`nothing matches ${sel}`);
      element.focus();
      const prototype = Object.getPrototypeOf(element);
      Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, text);
      element.dispatchEvent(new Event('input', { bubbles: true }));
      element.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    }, selector, value);
  }

  /**
   * Queues answers for the next dialogs, in order: a string answers a prompt, `true` accepts a
   * confirm/alert, `false` cancels. A dialog with no queued answer is cancelled and recorded as
   * an error (the test didn't expect it).
   */
  answerDialogs(...answers) {
    this.dialogAnswers.push(...answers);
  }

  /** Sets the files of an `<input type=file>` (paths on this machine), firing its change event. */
  async setFiles(selector, paths) {
    const { root } = await this.send('DOM.getDocument', { depth: 0 });
    const { nodeId } = await this.send('DOM.querySelector', { nodeId: root.nodeId, selector });
    if (!nodeId) throw new Error(`nothing matches ${selector}`);
    await this.send('DOM.setFileInputFiles', { nodeId, files: paths });
  }

  #answerDialog({ type, message }) {
    const answer = this.dialogAnswers.shift();
    if (answer === undefined) {
      this.errors.push(`unexpected ${type} dialog: ${message}`);
      this.send('Page.handleJavaScriptDialog', { accept: false }).catch(() => {});
      return;
    }
    const accept = answer !== false;
    const promptText = typeof answer === 'string' ? answer : undefined;
    this.send('Page.handleJavaScriptDialog', { accept, promptText }).catch(() => {});
  }

  async screenshot(path) {
    const { data } = await this.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    writeFileSync(path, Buffer.from(data, 'base64'));
  }
}

/** The browser writes its debugging port and path to `DevToolsActivePort` once it is ready. */
async function readEndpoint(profile) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const [port, path] = readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').trim().split('\n');
      if (port && path) return `ws://127.0.0.1:${port}${path}`;
    } catch {
      // not written yet
    }
    await sleep(100);
  }
  throw new Error('The browser did not start (no DevToolsActivePort).');
}
