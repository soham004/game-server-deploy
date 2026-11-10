/**
 * Autocomplete for the console's command field: suggestions under the text as it's typed, and the
 * command's shape above it (`give <targets> <item> [<count>]`, the part being typed in bold).
 *
 *   const assist = createCommandAssist({ instanceId, input });
 *   form.before(assist.element);   // the suggestions, the shape and the status line
 *   assist.prepare();              // asks the server to get its commands ready
 *   input.addEventListener('keydown', (event) => assist.handleKey(event) || history(event));
 *
 * Keys while the list is open: ↑/↓ choose, Tab or Enter take the choice (Tab the first one if
 * none is chosen), Esc closes. Ctrl+Space opens it. With the list closed, Tab completes with the
 * first suggestion for the text now; other keys do what they did. The list closes as the text
 * changes, so a key never takes a suggestion meant for the text before.
 * The server completes (`GET …/commands/complete`); answers to older text are dropped.
 */
import { api } from '../core/api.js';
import { applySuggestion, describeStatus, isPreparing, moveSelection } from '../core/command-input.js';
import { debounce } from '../core/debounce.js';
import { h } from '../core/dom.js';

const TYPING_PAUSE_MS = 80;
const STATUS_EVERY_MS = 2000;
/** Long enough for the data generator on a slow machine; then the status is left as it is. */
const STATUS_TRIES = 150;

const KIND_LABELS = {
  command: 'command',
  player: 'player',
  selector: 'selector',
  coordinates: 'position',
  id: 'id',
  tag: 'tag',
};

let listCount = 0;

export function createCommandAssist({ instanceId, input }) {
  const listId = `command-suggestions-${++listCount}`;
  const list = h('ul', { class: 'c-assist__list', id: listId, role: 'listbox', 'aria-label': 'Suggestions', hidden: true });
  const signature = h('p', { class: 'c-assist__signature', 'aria-live': 'polite' });
  const hint = h('p', { class: 'c-assist__hint c-muted' });
  const statusText = h('span');
  const retry = h('button', { class: 'c-button c-button--ghost c-button--small', type: 'button', hidden: true, onClick: () => prepare(true) }, 'Try again');
  const status = h('p', { class: 'c-assist__status c-muted', role: 'status', hidden: true }, statusText, ' ', retry);
  // The list opens upwards from above the shape and hint, so they stay in view.
  const element = h('div', { class: 'c-assist' }, list, signature, hint, status);

  input.setAttribute('role', 'combobox');
  input.setAttribute('aria-autocomplete', 'list');
  input.setAttribute('aria-controls', listId);
  input.setAttribute('aria-expanded', 'false');

  let suggestions = [];
  let selected = -1;
  /** Numbers each request, so an older answer can't replace a newer one. */
  let latest = 0;
  /** After Esc the list stays shut until the text changes. */
  let dismissed = false;
  let statusTimer = null;
  let statusTries = 0;
  let destroyed = false;

  const later = debounce(() => refresh(), TYPING_PAUSE_MS);
  input.addEventListener('input', () => {
    dismissed = false;
    // What's listed was for the text before: it goes, so a key can't take a suggestion that no
    // longer fits, and comes back once the server has answered for this text.
    close();
    later();
  });
  // The cursor moving changes what's being typed.
  input.addEventListener('click', () => later());
  input.addEventListener('keyup', (event) => {
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight' || event.key === 'Home' || event.key === 'End') later();
  });
  input.addEventListener('blur', () => close());
  input.addEventListener('focus', () => {
    if (input.value) later();
  });

  /** Asks for suggestions for the text now; `thenAccept` takes the first one (Tab while closed). */
  async function refresh({ thenAccept = false } = {}) {
    if (destroyed || input.disabled) return;
    const request = ++latest;
    const text = input.value;
    const cursor = input.selectionStart ?? text.length;
    if (!text.trim() && !list.hidden) {
      close();
    }
    try {
      const query = new URLSearchParams({ input: text, cursor: String(cursor) });
      const completion = await api.get(`/instances/${instanceId}/commands/complete?${query}`);
      if (request !== latest || destroyed) return;
      showSignature(completion.signature);
      suggestions = text.trim() || document.activeElement === input ? completion.suggestions : [];
      selected = -1;
      if (thenAccept && suggestions.length > 0) {
        accept(0);
        return;
      }
      render();
    } catch {
      // Autocomplete is a help, not a requirement: typing goes on without it.
      if (request === latest) close();
    }
  }

  function render() {
    const open = suggestions.length > 0 && !dismissed && document.activeElement === input;
    list.replaceChildren(...(open ? suggestions.map(option) : []));
    list.hidden = !open;
    input.setAttribute('aria-expanded', String(open));
    const active = open && selected >= 0 ? `${listId}-${selected}` : null;
    if (active) {
      input.setAttribute('aria-activedescendant', active);
      list.children[selected]?.scrollIntoView({ block: 'nearest' });
    } else {
      input.removeAttribute('aria-activedescendant');
    }
  }

  function option(suggestion, index) {
    const kind = KIND_LABELS[suggestion.kind];
    return h('li', {
      class: `c-assist__option${index === selected ? ' is-selected' : ''}`,
      id: `${listId}-${index}`,
      role: 'option',
      'aria-selected': String(index === selected),
      // Keeps the focus in the field, so taking a suggestion doesn't close the list first.
      onMousedown: (event) => event.preventDefault(),
      onClick: () => accept(index),
    },
    h('span', { class: 'c-assist__text' }, suggestion.text),
    suggestion.detail ? h('span', { class: 'c-assist__detail c-muted' }, suggestion.detail) : null,
    kind ? h('span', { class: 'c-assist__kind c-muted' }, kind) : null);
  }

  function showSignature(shape) {
    if (!shape) {
      signature.replaceChildren();
      hint.textContent = '';
      return;
    }
    const parts = shape.parts.map((part) => (part.current ? h('strong', null, part.text) : part.text));
    signature.replaceChildren(...parts.flatMap((part, index) => (index ? [' ', part] : [part])));
    hint.textContent = shape.hint ?? '';
  }

  function accept(index) {
    const suggestion = suggestions[index];
    if (!suggestion) return;
    const { value, cursor } = applySuggestion(input.value, suggestion);
    input.value = value;
    input.setSelectionRange(cursor, cursor);
    close();
    input.focus();
    refresh();
  }

  function close() {
    suggestions = [];
    selected = -1;
    render();
  }

  /** Handles a key in the field; returns true when it was autocomplete's. */
  function handleKey(event) {
    if (event.key === ' ' && event.ctrlKey) {
      event.preventDefault();
      dismissed = false;
      refresh();
      return true;
    }
    if (list.hidden) {
      // Tab before the list came: ask now and take the first (moving the focus away from the
      // console's field with Tab is no use).
      if (event.key !== 'Tab' || event.shiftKey || !input.value.trim()) return false;
      event.preventDefault();
      later.cancel();
      refresh({ thenAccept: true });
      return true;
    }
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp':
        selected = moveSelection(selected, suggestions.length, event.key === 'ArrowDown' ? 1 : -1);
        render();
        break;
      case 'Tab':
        accept(selected >= 0 ? selected : 0);
        break;
      case 'Enter':
        // Enter sends the command unless a suggestion was chosen.
        if (selected < 0) return false;
        accept(selected);
        break;
      case 'Escape':
        dismissed = true;
        close();
        break;
      default:
        return false;
    }
    event.preventDefault();
    return true;
  }

  function showStatus(next) {
    const message = describeStatus(next);
    status.hidden = !message;
    statusText.textContent = message?.text ?? '';
    retry.hidden = !message?.retry;
    clearTimeout(statusTimer);
    if (isPreparing(next) && statusTries < STATUS_TRIES && !destroyed) {
      statusTries += 1;
      statusTimer = setTimeout(async () => {
        try {
          showStatus(await api.get(`/instances/${instanceId}/commands`));
        } catch {
          status.hidden = true;
        }
      }, STATUS_EVERY_MS);
    }
  }

  /** Gets the server's commands ready (the generator once per version, `help` per set of mods). */
  async function prepare(again = false) {
    statusTries = 0;
    try {
      showStatus(await api.post(`/instances/${instanceId}/commands/prepare`, { retry: again }));
    } catch {
      status.hidden = true;
    }
  }

  return {
    element,
    prepare,
    handleKey,
    /** After the command was sent: nothing typed, nothing to show. */
    reset() {
      latest += 1;
      later.cancel();
      close();
      showSignature(null);
    },
    destroy() {
      destroyed = true;
      later.cancel();
      clearTimeout(statusTimer);
    },
  };
}
