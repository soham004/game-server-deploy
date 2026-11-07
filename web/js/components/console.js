/**
 * Live server console.
 *
 *   const console = createConsole({ instanceId, canWrite });
 *   root.append(console.element);
 *   console.setRunning(true);   // enables the input
 *   console.destroy();          // on unmount
 *
 * Lines come from the REST snapshot plus the `console:<id>` live topic; sequence numbers
 * de-duplicate the overlap. The view follows new output unless the user scrolled up.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { clockTime } from '../core/format.js';
import { live } from '../core/ws.js';

const MAX_LINES = 2000;
const HISTORY_LIMIT = 100;

export function createConsole({ instanceId, canWrite }) {
  let lastSeq = 0;
  let follow = true;
  let running = false;
  const history = loadHistory(instanceId);
  let historyIndex = history.length;

  const lines = h('ol', { class: 'c-console__lines', role: 'log', 'aria-live': 'off', 'aria-label': 'Server console' });
  const jump = h('button', { class: 'c-button c-console__jump', type: 'button', hidden: true }, 'Jump to latest');
  const input = h('input', {
    class: 'c-input c-console__input',
    type: 'text',
    autocomplete: 'off',
    spellcheck: 'false',
    'aria-label': 'Console command',
    placeholder: 'Start the server to send commands',
    disabled: true,
  });
  const form = h('form', { class: 'c-console__form' },
    h('span', { class: 'c-console__prompt', 'aria-hidden': 'true' }, '>'),
    input,
    h('button', { class: 'c-button c-button--primary', type: 'submit', disabled: true }, 'Send'),
  );
  const element = h('section', { class: 'c-console' },
    h('div', { class: 'c-console__viewport' }, lines, jump),
    canWrite ? form : null,
  );

  const viewport = element.querySelector('.c-console__viewport');
  viewport.addEventListener('scroll', () => {
    follow = viewport.scrollTop + viewport.clientHeight >= viewport.scrollHeight - 40;
    jump.hidden = follow;
  });
  jump.addEventListener('click', () => {
    follow = true;
    jump.hidden = true;
    scrollToEnd();
  });

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const command = input.value.trim();
    if (!command) return;
    try {
      await api.post(`/instances/${instanceId}/command`, { command });
      remember(command);
      input.value = '';
    } catch (error) {
      appendLocal(`Could not send: ${error.message}`);
    }
  });
  input.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowUp' && historyIndex > 0) {
      historyIndex -= 1;
      input.value = history[historyIndex];
      event.preventDefault();
    } else if (event.key === 'ArrowDown') {
      historyIndex = Math.min(history.length, historyIndex + 1);
      input.value = history[historyIndex] ?? '';
      event.preventDefault();
    }
  });

  function append(line) {
    if (line.seq <= lastSeq) return;
    lastSeq = line.seq;
    lines.append(lineElement(line));
    while (lines.childElementCount > MAX_LINES) lines.firstElementChild.remove();
    if (follow) scrollToEnd();
  }

  function appendLocal(text) {
    lines.append(h('li', { class: 'c-console__line is-system is-error' }, text));
    if (follow) scrollToEnd();
  }

  function scrollToEnd() {
    viewport.scrollTop = viewport.scrollHeight;
  }

  function remember(command) {
    if (history[history.length - 1] !== command) history.push(command);
    while (history.length > HISTORY_LIMIT) history.shift();
    historyIndex = history.length;
    try {
      window.localStorage.setItem(historyKey(instanceId), JSON.stringify(history));
    } catch {
      // Storage may be unavailable (private mode); history just won't persist.
    }
  }

  async function loadSnapshot() {
    try {
      const snapshot = await api.get(`/instances/${instanceId}/console`);
      for (const line of snapshot.console) append(line);
      return snapshot;
    } catch (error) {
      appendLocal(`Could not load the console: ${error.message}`);
      return null;
    }
  }

  // Subscribe first so nothing is missed between the snapshot and the stream.
  const offLive = live.on(`console:${instanceId}`, (event) => append(event.data));
  const offResync = live.onResync(loadSnapshot);
  const ready = loadSnapshot();

  return {
    element,
    ready,
    setRunning(isRunning) {
      running = isRunning;
      input.disabled = !running;
      form.querySelector('[type="submit"]').disabled = !running;
      input.placeholder = running ? 'Type a command (without /) and press Enter' : 'Start the server to send commands';
    },
    destroy() {
      offLive();
      offResync();
    },
  };
}

function lineElement(line) {
  const classes = ['c-console__line', `is-${line.source}`];
  if (line.level === 'warn') classes.push('is-warn');
  if (line.level === 'error' || line.level === 'fatal') classes.push('is-error');
  const text = line.source === 'stdout' || line.source === 'stderr' ? line.raw : line.text;
  return h('li', { class: classes.join(' ') },
    line.source === 'system' || line.source === 'input'
      ? h('span', { class: 'c-console__time' }, clockTime(line.received_at), ' ')
      : null,
    text,
  );
}

function historyKey(instanceId) {
  return `console-history:${instanceId}`;
}

function loadHistory(instanceId) {
  try {
    const stored = JSON.parse(window.localStorage.getItem(historyKey(instanceId)) ?? '[]');
    return Array.isArray(stored) ? stored.filter((item) => typeof item === 'string') : [];
  } catch {
    return [];
  }
}
