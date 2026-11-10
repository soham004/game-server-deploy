# CodeMirror vendor build

The web UI's code editor is CodeMirror 6, loaded as one prebuilt ES module, `web/vendor/codemirror.js`
(D-008). This folder makes that file; the app itself has no build step and never runs npm.

```sh
cd tools/codemirror
npm ci            # exact versions from package-lock.json
npm run build     # writes web/vendor/codemirror.js and codemirror.LICENSES.txt
```

- Versions are pinned exactly in `package.json`, and the lockfile pins everything they bring. Update by
  changing a version, `npm install`, `npm run build`, then check the editor in the browser suite
  (`tests/e2e/ui.e2e.mjs`), which also checks for Content-Security-Policy violations.
- `entry.mjs` lists what the UI imports; `web/js/core/code-editor.js` is the only module that imports it.
- `node_modules/` is not committed.
