/**
 * A server's log history: the files on the left (the current log first), the chosen one on the
 * right, with its last lines first and older ones on request. A crash report gets its gist on top.
 *
 *   const logs = createLogs({ instanceId });
 *   panel.append(logs.element);
 *   logs.load();              // on opening the tab
 *   logs.open(name);          // shows one file
 *
 *   const crash = createLastCrash({ instanceId, onOpen: (name) => … });
 *   overview.append(crash.element);
 *   crash.refresh(instance);  // shows the last crash while the server is down because of it
 */
import { api } from '../core/api.js';
import { debounce } from '../core/debounce.js';
import { h } from '../core/dom.js';
import { relativeTime, sizeLabel } from '../core/format.js';

const TYPING_PAUSE_MS = 300;

const KIND_LABELS = {
  latest: 'Current log',
  archive: 'Earlier run',
  debug: 'Debug log',
  crash: 'Crash report',
  jvm_crash: 'JVM crash',
};

const LEVELS = [
  ['', 'All levels'],
  ['info', 'Info and above'],
  ['warn', 'Warnings and errors'],
  ['error', 'Errors only'],
];

const url = (instanceId, name, rest = '') => `/instances/${instanceId}/logs/${encodeURIComponent(name)}${rest}`;

/** What a crash report's summary says, as a card. */
function summaryCard({ description, time, error, frames, suspected, first_outside_code: outside }) {
  return h('section', { class: 'c-logs__crash' },
    h('h3', { class: 'c-logs__crash-title' }, description ?? 'Crash report'),
    time ? h('p', { class: 'c-muted' }, time) : null,
    error ? h('p', null, h('code', null, error)) : null,
    suspected.length ? h('p', null, h('strong', null, 'Suspected by the loader: '), suspected.join('; ')) : null,
    outside ? h('p', null, h('strong', null, 'First code outside the game: '), h('code', null, outside)) : null,
    frames.length ? h('details', null, h('summary', null, 'Where it happened'),
      h('ul', { class: 'c-logs__frames' }, frames.map((frame) => h('li', null, h('code', null, frame))))) : null,
  );
}

export function createLogs({ instanceId }) {
  const files = h('ul', { class: 'c-logs__files' });
  const search = h('input', { class: 'c-input', type: 'search', name: 'log_query', placeholder: 'Search this log', 'aria-label': 'Search this log' });
  const regex = h('input', { type: 'checkbox', name: 'log_regex' });
  const level = h('select', { class: 'c-input', name: 'log_level', 'aria-label': 'Level' },
    LEVELS.map(([value, label]) => h('option', { value }, label)));
  const wrap = h('input', { type: 'checkbox', name: 'log_wrap', checked: true });
  const download = h('a', { class: 'c-button c-button--ghost', hidden: true }, 'Download');
  const refreshButton = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => load() }, 'Refresh');
  const summary = h('div', { class: 'c-logs__summary' });
  const earlier = h('button', { class: 'c-button c-button--ghost c-logs__earlier', type: 'button', hidden: true, onClick: () => loadEarlier() }, 'Show earlier lines');
  const lines = h('ol', { class: 'c-logs__lines is-wrapped', 'aria-label': 'Log' });
  const status = h('p', { class: 'c-muted c-logs__status', role: 'status' });
  const viewport = h('div', { class: 'c-logs__viewport' }, earlier, lines);
  const element = h('section', { class: 'c-logs' },
    h('div', { class: 'c-logs__side' }, refreshButton, files),
    h('div', { class: 'c-logs__main' },
      h('form', { class: 'c-logs__tools', onSubmit: (event) => { event.preventDefault(); show(); } },
        search,
        h('label', { class: 'c-check' }, regex, h('span', null, 'Regex')),
        level,
        h('label', { class: 'c-check' }, wrap, h('span', null, 'Wrap')),
        download),
      summary, status, viewport));

  let chosen = null;
  let earlierLine = null;
  /** Numbers each read, so an older answer can't replace a newer one. */
  let latestRead = 0;

  const query = (before) => {
    const params = new URLSearchParams();
    if (search.value) params.set('q', search.value);
    if (regex.checked) params.set('regex', 'true');
    if (level.value) params.set('level', level.value);
    if (before) params.set('before', String(before));
    const text = params.toString();
    return text ? `?${text}` : '';
  };

  const lineElement = (line) => h('li', { class: `c-logs__line is-${line.level}`, dataset: { line: line.number } },
    h('span', { class: 'c-logs__number', 'aria-hidden': 'true' }, String(line.number)),
    h('span', { class: 'c-logs__text' }, line.text));

  /** Lists the files, and keeps showing the chosen one if it is still there. */
  async function load() {
    let list;
    try {
      list = await api.get(`/instances/${instanceId}/logs`);
    } catch (error) {
      files.replaceChildren(h('li', { class: 'c-muted' }, error.message));
      return;
    }
    if (list.length === 0) {
      files.replaceChildren(h('li', { class: 'c-muted' }, 'No logs yet: they appear once the server has run.'));
      return;
    }
    if (!list.some((file) => file.name === chosen)) chosen = list[0].name;
    files.replaceChildren(...list.map(fileRow));
    await show();
  }

  function fileRow(file) {
    return h('li', { class: 'c-logs__file' },
      h('button', {
        class: `c-logs__file-button is-${file.kind}${file.name === chosen ? ' is-active' : ''}`,
        type: 'button',
        dataset: { log: file.name },
        onClick: () => open(file.name),
      },
      h('span', { class: 'c-logs__file-name' }, file.name),
      h('span', { class: 'c-muted c-logs__file-meta' },
        `${KIND_LABELS[file.kind] ?? file.kind} · ${sizeLabel(file.size)}${file.modified ? ` · ${relativeTime(file.modified)}` : ''}`)));
  }

  /** Shows one file. The list is read again: this file can be newer than it, like a fresh crash report. */
  function open(name) {
    chosen = name;
    return load();
  }

  /** Reads the last lines of the chosen file. */
  async function show() {
    if (!chosen) return;
    const thisRead = ++latestRead;
    download.href = `/api/v1${url(instanceId, chosen, '/download')}`;
    download.hidden = false;
    try {
      const page = await api.get(url(instanceId, chosen, query()));
      if (thisRead !== latestRead) return;
      summary.replaceChildren(...(page.crash ? [summaryCard(page.crash)] : []));
      lines.replaceChildren(...page.lines.map(lineElement));
      describe(page);
      viewport.scrollTop = viewport.scrollHeight;
    } catch (error) {
      if (thisRead !== latestRead) return;
      lines.replaceChildren();
      summary.replaceChildren();
      earlier.hidden = true;
      status.textContent = error.message;
    }
  }

  async function loadEarlier() {
    const thisRead = latestRead;
    earlier.disabled = true;
    try {
      const page = await api.get(url(instanceId, chosen, query(earlierLine)));
      if (thisRead !== latestRead) return;
      // The view stays on the lines it was showing.
      const fromEnd = viewport.scrollHeight - viewport.scrollTop;
      lines.prepend(...page.lines.map(lineElement));
      viewport.scrollTop = viewport.scrollHeight - fromEnd;
      describe(page);
    } catch (error) {
      status.textContent = error.message;
    } finally {
      earlier.disabled = false;
    }
  }

  function describe(page) {
    earlierLine = page.earlier;
    earlier.hidden = page.earlier === null;
    const count = lines.childElementCount;
    const parts = [count === 0 ? 'No lines match.' : `${count.toLocaleString()} ${count === 1 ? 'line' : 'lines'} shown.`];
    if (page.truncated) parts.push('This file is damaged or too big, so this isn’t all of it.');
    status.textContent = parts.join(' ');
  }

  const searchSoon = debounce(() => show(), TYPING_PAUSE_MS);
  search.addEventListener('input', () => searchSoon());
  regex.addEventListener('change', () => show());
  level.addEventListener('change', () => show());
  wrap.addEventListener('change', () => lines.classList.toggle('is-wrapped', wrap.checked));

  return { element, load, open };
}

export function createLastCrash({ instanceId, onOpen }) {
  const element = h('section', { class: 'c-overview__section', hidden: true });
  /** The status last looked at, so state updates don't read the files again. */
  let lookedAt = null;

  async function refresh(instance) {
    const { status, exit } = instance.state;
    const crashed = status === 'crashed' || (status === 'stopped' && exit && exit.code !== 0 && exit.code !== null);
    if (!crashed) {
      element.hidden = true;
      lookedAt = null;
      return;
    }
    const key = `${status}:${instance.state.since}`;
    if (key === lookedAt) return;
    lookedAt = key;
    try {
      const list = await api.get(`/instances/${instanceId}/logs`);
      const report = list.find((file) => ['crash', 'jvm_crash'].includes(file.kind));
      if (!report) return;
      const { crash } = await api.get(url(instanceId, report.name, '?limit=1'));
      element.replaceChildren(
        h('h2', { class: 'c-overview__heading' }, 'Last crash'),
        crash ? summaryCard(crash) : '',
        h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => onOpen(report.name) }, 'Open the report'),
      );
      element.hidden = false;
    } catch {
      // Being unable to look (no right to read logs, say) just means no card.
      element.hidden = true;
    }
  }

  return { element, refresh };
}
