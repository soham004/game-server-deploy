// What the web UI uses of CodeMirror 6, re-exported from one module (see README.md). Anything new
// the editor needs is added here, then `npm run build`.

export { Compartment, EditorSelection, EditorState, StateEffect, StateField } from '@codemirror/state';
export {
  Decoration,
  EditorView,
  ViewPlugin,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  placeholder,
  rectangularSelection,
} from '@codemirror/view';
export { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
export {
  LanguageSupport,
  StreamLanguage,
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from '@codemirror/language';
export { classHighlighter, tags } from '@lezer/highlight';
export { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
export { highlightSelectionMatches, openSearchPanel, search, searchKeymap } from '@codemirror/search';
export { forEachDiagnostic, lintGutter, lintKeymap, linter, setDiagnostics } from '@codemirror/lint';
export { json } from '@codemirror/lang-json';
export { yaml } from '@codemirror/lang-yaml';
export { toml } from '@codemirror/legacy-modes/mode/toml';
export { properties } from '@codemirror/legacy-modes/mode/properties';
export { getChunks, unifiedMergeView } from '@codemirror/merge';
