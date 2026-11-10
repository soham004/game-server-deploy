/**
 * The file manager's editor: one text file in the code editor, with syntax checks for JSON, YAML
 * and TOML, the changes since it was opened, and what to do when the file changed on disk meanwhile.
 *
 *   const editor = createFileEditor({ base: `/instances/${id}/files`, canWrite, notice, onSaved });
 *   panel.append(editor.element);
 *   editor.show(file);   // a `GET …/files/text` answer: { path, content, revision }
 *
 * Checks (`POST …/files/check`) run as the text changes and before saving. Saving text with a mistake
 * asks first, unless the file had one when it was opened (a mod may accept what the format doesn't,
 * such as comments in JSON): then it's only shown.
 */
import { api } from '../core/api.js';
import { createCodeEditor } from '../core/code-editor.js';
import { checkedFormat } from '../core/code-languages.js';
import { debounce } from '../core/debounce.js';
import { h } from '../core/dom.js';
import { toast } from '../core/toast.js';

const CHECK_PAUSE_MS = 500;

/** "Line 3, column 5: expected `,` or `}`". */
export function describeProblem({ line, column, message }) {
  return `Line ${line}, column ${column}: ${message}`;
}

export function createFileEditor({ base, canWrite, notice = () => '', onSaved = () => {} }) {
  let file = null;
  let editor = null;
  let openedWithProblem = false;
  let diffShown = false;
  /** Numbers each check, so an older answer can't replace a newer one. */
  let latestCheck = 0;

  const title = h('h2', { class: 'c-editor__title' });
  const status = h('span', { class: 'c-editor__status c-muted' });
  const saveButton = h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => save(), hidden: !canWrite }, 'Save');
  const changesButton = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => toggleDiff() }, 'Show changes');
  const searchButton = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => editor?.openSearch() }, 'Find');
  const close = h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => hide() }, 'Close');
  const problems = h('p', { class: 'c-editor__problems', role: 'status', hidden: true });
  const conflict = h('div', { class: 'c-editor__conflict c-banner', role: 'alert', hidden: true });
  const body = h('div', { class: 'c-editor__body' });
  const element = h('section', { class: 'c-editor', hidden: true, 'aria-label': 'File editor' },
    h('header', { class: 'c-editor__header' }, title, status, searchButton, changesButton, saveButton, close),
    conflict, problems, body);

  const checkSoon = debounce(() => runCheck(), CHECK_PAUSE_MS);

  function dirty() {
    return Boolean(file && editor && editor.value !== file.content);
  }

  async function show(next) {
    if (dirty() && !window.confirm('Discard unsaved changes?')) return;
    editor?.destroy();
    file = next;
    diffShown = false;
    changesButton.textContent = 'Show changes';
    conflict.hidden = true;
    title.textContent = file.path;
    status.textContent = notice(file.path);
    element.hidden = false;
    editor = await createCodeEditor({
      value: file.content,
      path: file.path,
      readOnly: !canWrite,
      label: `Contents of ${file.path}`,
      onSave: () => save(),
      onChange: () => {
        status.textContent = dirty() ? 'Unsaved changes' : '';
        checkSoon();
      },
    });
    body.replaceChildren(editor.element);
    const found = await runCheck();
    openedWithProblem = found.length > 0;
    editor.focus();
  }

  function hide() {
    if (dirty() && !window.confirm('Discard unsaved changes?')) return;
    checkSoon.cancel();
    editor?.destroy();
    editor = null;
    file = null;
    body.replaceChildren();
    element.hidden = true;
  }

  /** Checks the text now; marks and lists what's wrong. Returns the problems. */
  async function runCheck() {
    if (!file || !editor || !checkedFormat(file.path)) {
      problems.hidden = true;
      return [];
    }
    const thisCheck = ++latestCheck;
    let found = [];
    try {
      found = (await api.post(`${base}/check`, { path: file.path, content: editor.value })).problems;
    } catch {
      // Checking is a help: if it can't run, saving still works.
    }
    if (thisCheck !== latestCheck || !editor) return found;
    editor.setProblems(found);
    problems.hidden = found.length === 0;
    problems.textContent = found.map(describeProblem).join(' ');
    return found;
  }

  function toggleDiff() {
    if (!editor || !file) return;
    diffShown = !diffShown;
    if (diffShown) editor.showDiff(file.content);
    else editor.hideDiff();
    changesButton.textContent = diffShown ? 'Hide changes' : 'Show changes';
  }

  async function save({ revision = file?.revision } = {}) {
    if (!file || !editor || !canWrite) return;
    checkSoon.cancel();
    const found = await runCheck();
    if (found.length > 0 && !openedWithProblem) {
      const go = window.confirm(`${file.path} has a mistake: ${describeProblem(found[0])}.\n\nSave it anyway?`);
      if (!go) return;
    }
    const content = editor.value;
    try {
      file = await api.put(`${base}/text`, { path: file.path, content, revision });
      openedWithProblem = found.length > 0;
      conflict.hidden = true;
      if (diffShown) toggleDiff();
      status.textContent = 'Saved';
      onSaved(file);
    } catch (error) {
      if (error.code === 'file_changed') {
        await showConflict();
      } else {
        toast(error.message, { kind: 'error' });
      }
    }
  }

  /**
   * The file changed on disk since it was opened (the server, or someone else, wrote it). Shows how
   * this text differs from the file as it is now; the operator keeps one or the other.
   */
  async function showConflict() {
    let disk;
    try {
      disk = await api.get(`${base}/text?path=${encodeURIComponent(file.path)}`);
    } catch (error) {
      toast(`${file.path} changed on disk, and it can't be read again: ${error.message}`, { kind: 'error' });
      return;
    }
    editor.showDiff(disk.content);
    diffShown = true;
    changesButton.textContent = 'Hide changes';
    conflict.replaceChildren(
      h('p', null, `${file.path} changed on disk since you opened it. The marked lines are how your text differs from it now.`),
      h('div', { class: 'c-editor__conflict-actions' },
        h('button', { class: 'c-button c-button--primary', type: 'button', onClick: () => save({ revision: disk.revision }) }, 'Save mine over it'),
        h('button', { class: 'c-button', type: 'button', onClick: () => useDisk(disk) }, 'Load the version on disk')),
    );
    conflict.hidden = false;
  }

  function useDisk(disk) {
    file = disk;
    editor.hideDiff();
    diffShown = false;
    changesButton.textContent = 'Show changes';
    editor.setValue(disk.content);
    conflict.hidden = true;
    status.textContent = '';
    runCheck();
  }

  return { element, show, hide };
}
