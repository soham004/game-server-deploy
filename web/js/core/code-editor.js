/**
 * The code editor: CodeMirror 6, loaded from the one vendored file (`/vendor/codemirror.js`, built
 * by tools/codemirror) the first time an editor opens. If it can't load, a plain text area with the
 * same methods stands in, so editing never depends on it.
 *
 *   const editor = await createCodeEditor({ value: text, path: 'config/x.json', onSave, onChange });
 *   panel.append(editor.element);
 *   editor.value;                       // the text now
 *   editor.setValue(text);              // replaces it
 *   editor.setProblems([{ line: 3, column: 5, message: 'expected ,' }]);
 *   editor.showDiff(original);          // the changes against `original`, inline; hideDiff() ends it
 *
 * Read-only viewers (logs) pass `readOnly`, `lineNumber` (the file's numbers for the shown lines)
 * and `lineClass` (a class for each line, e.g. its log level).
 *
 * Each editor lives in a shadow root (`element` is its host), with css/components/code-editor.css
 * adopted inside: our styles for CodeMirror (`cm-*`, `tok-*`). CodeMirror's own
 * base styles go in through constructed style sheets, which the Content-Security-Policy allows.
 */
import { h } from './dom.js';
import { languageFor, log, mcfunction, snbt } from './code-languages.js';

const STYLES = '/css/components/code-editor.css';

let loading = null;
let ourSheet = null;

/** The CodeMirror module, loaded once. */
export function loadCodeMirror() {
  loading ??= import('/vendor/codemirror.js');
  return loading;
}

/**
 * Our styles for the editor as one constructed sheet, shared by every editor. Adopted after
 * CodeMirror's own sheet, so ours win where both style the same thing (a `<link>` in the shadow
 * root would come before it, and lose).
 */
function editorStyles() {
  ourSheet ??= fetch(STYLES)
    .then((response) => (response.ok ? response.text() : ''))
    .then((text) => {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(text);
      return sheet;
    });
  return ourSheet;
}

export async function createCodeEditor(options = {}) {
  let cm;
  let sheet;
  try {
    [cm, sheet] = await Promise.all([loadCodeMirror(), editorStyles()]);
  } catch {
    return plainEditor(options);
  }
  return codeMirrorEditor(cm, sheet, options);
}

function languageExtension(cm, language) {
  switch (language) {
    case 'json': return cm.json();
    case 'yaml': return cm.yaml();
    case 'toml': return cm.StreamLanguage.define(cm.toml);
    case 'properties': return cm.StreamLanguage.define(cm.properties);
    case 'mcfunction': return cm.StreamLanguage.define(mcfunction);
    case 'snbt': return cm.StreamLanguage.define(snbt);
    case 'log': return cm.StreamLanguage.define(log);
    default: return [];
  }
}

function codeMirrorEditor(cm, sheet, {
  value = '',
  path = '',
  language = languageFor(path),
  readOnly = false,
  wrap = false,
  label = 'File contents',
  onChange,
  onSave,
  lineNumber,
  lineClass,
} = {}) {
  const editable = new cm.Compartment();
  const wrapping = new cm.Compartment();
  const diff = new cm.Compartment();
  let lineClasses = lineClass;

  const extensions = [
    cm.lineNumbers(lineNumber ? { formatNumber: (n) => String(lineNumber(n) ?? n) } : {}),
    cm.highlightActiveLineGutter(),
    cm.highlightSpecialChars(),
    cm.history(),
    cm.foldGutter(),
    cm.drawSelection(),
    cm.indentOnInput(),
    cm.syntaxHighlighting(cm.classHighlighter),
    cm.bracketMatching(),
    cm.closeBrackets(),
    cm.highlightActiveLine(),
    cm.highlightSelectionMatches(),
    cm.search({ top: true }),
    cm.lintGutter(),
    cm.keymap.of([
      { key: 'Mod-s', preventDefault: true, run: () => { onSave?.(); return true; } },
      ...cm.closeBracketsKeymap,
      ...cm.defaultKeymap,
      ...cm.searchKeymap,
      ...cm.historyKeymap,
      ...cm.foldKeymap,
      ...cm.lintKeymap,
      // Tab indents; Esc then Tab leaves the editor (CodeMirror's rule for keyboard users).
      cm.indentWithTab,
    ]),
    languageExtension(cm, language),
    editable.of(readOnlyState(cm, readOnly)),
    wrapping.of(wrap ? cm.EditorView.lineWrapping : []),
    diff.of([]),
    // CodeMirror owns the editor's class attribute: ours goes through it.
    cm.EditorView.editorAttributes.of({ class: 'c-code' }),
    cm.EditorView.contentAttributes.of({ 'aria-label': label }),
    cm.EditorView.updateListener.of((update) => {
      if (update.docChanged) onChange?.(update.state.doc.toString());
    }),
    lineClassPlugin(cm, () => lineClasses),
  ];
  // In a shadow root: CodeMirror then adds its base styles as a constructed style sheet, which the
  // Content-Security-Policy allows; in the page itself it would add a <style> element, which it
  // blocks. Our own styles for it are linked inside; the colour tokens inherit through.
  const host = h('div', { class: 'c-code-host' });
  const shadow = host.attachShadow({ mode: 'open' });
  const view = new cm.EditorView({ doc: value, extensions, root: shadow, parent: shadow });
  shadow.adoptedStyleSheets = [...shadow.adoptedStyleSheets.filter((s) => s !== sheet), sheet];

  return {
    element: host,
    view,
    get value() {
      return view.state.doc.toString();
    },
    setValue(text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    },
    /** Adds lines at the top, keeping the lines that were showing in view (older log lines). */
    prepend(text) {
      const firstShown = view.lineBlockAtHeight(view.scrollDOM.scrollTop).from;
      view.dispatch({ changes: { from: 0, insert: text } });
      view.dispatch({ effects: cm.EditorView.scrollIntoView(firstShown + text.length, { y: 'start' }) });
    },
    setLineClass(fn) {
      lineClasses = fn;
      view.dispatch({});
    },
    setReadOnly(on) {
      view.dispatch({ effects: editable.reconfigure(readOnlyState(cm, on)) });
    },
    setWrap(on) {
      view.dispatch({ effects: wrapping.reconfigure(on ? cm.EditorView.lineWrapping : []) });
    },
    /** Marks problems ({ line, column, message }, both from 1) in the text, or clears them. */
    setProblems(problems) {
      const doc = view.state.doc;
      const diagnostics = problems.map(({ line, column, message }) => {
        const at = doc.line(Math.min(Math.max(line, 1), doc.lines));
        const from = Math.min(at.from + Math.max(column - 1, 0), at.to);
        return { from, to: Math.min(Math.max(from + 1, at.to), doc.length), severity: 'error', message };
      });
      view.dispatch(cm.setDiagnostics(view.state, diagnostics));
    },
    /** Shows what changed against `original`, inline (deleted lines in red above the new ones). */
    showDiff(original) {
      view.dispatch({ effects: diff.reconfigure(cm.unifiedMergeView({ original, mergeControls: false, gutter: true })) });
    },
    hideDiff() {
      view.dispatch({ effects: diff.reconfigure([]) });
    },
    /** How many blocks of changes the diff shows (0 when none or no diff is shown). */
    changeCount() {
      return cm.getChunks(view.state)?.chunks.length ?? 0;
    },
    scrollToEnd() {
      view.dispatch({ effects: cm.EditorView.scrollIntoView(view.state.doc.length, { y: 'end' }) });
    },
    openSearch() {
      cm.openSearchPanel(view);
    },
    focus() {
      view.focus();
    },
    destroy() {
      view.destroy();
    },
  };
}

function readOnlyState(cm, readOnly) {
  return [cm.EditorState.readOnly.of(readOnly), cm.EditorView.editable.of(!readOnly)];
}

/**
 * A class on each line, from `lineClass(lineNumber)` (log levels). Every line gets one, not just
 * those on screen (CodeMirror also draws lines it keeps for the cursor); rebuilt when the text
 * changes, or when `setLineClass` asks with an empty transaction.
 */
function lineClassPlugin(cm, current) {
  const build = (state) => {
    const fn = current();
    if (!fn) return cm.Decoration.none;
    const ranges = [];
    for (let number = 1; number <= state.doc.lines; number += 1) {
      const name = fn(number);
      if (name) ranges.push(cm.Decoration.line({ class: name }).range(state.doc.line(number).from));
    }
    return cm.Decoration.set(ranges);
  };
  return cm.ViewPlugin.define((view) => ({
    decorations: build(view.state),
    update(update) {
      const asked = update.transactions.some((transaction) => !transaction.docChanged && !transaction.selection && transaction.effects.length === 0);
      if (update.docChanged || asked) this.decorations = build(update.state);
    },
  }), { decorations: (plugin) => plugin.decorations });
}

/** A text area with the editor's methods, for when CodeMirror can't load. */
function plainEditor({ value = '', readOnly = false, label = 'File contents', onChange, onSave, wrap = true } = {}) {
  const area = h('textarea', {
    class: 'c-input c-code c-code--plain', spellcheck: 'false', 'aria-label': label, readOnly,
    onInput: () => onChange?.(area.value),
    onKeydown: (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key === 's') { event.preventDefault(); onSave?.(); }
    },
  });
  area.value = value;
  area.wrap = wrap ? 'soft' : 'off';
  return {
    element: area,
    view: null,
    get value() { return area.value; },
    setValue(text) { area.value = text; },
    prepend(text) { area.value = text + area.value; },
    setLineClass() {},
    setReadOnly(on) { area.readOnly = on; },
    setWrap(on) { area.wrap = on ? 'soft' : 'off'; },
    setProblems() {},
    showDiff() {},
    hideDiff() {},
    changeCount: () => 0,
    scrollToEnd() { area.scrollTop = area.scrollHeight; },
    openSearch() {},
    focus() { area.focus(); },
    destroy() {},
  };
}
