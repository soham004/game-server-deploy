# Feature: File Manager (browse and edit server files in the browser)

## Path safety (the most important part): `gsd-core/files/safe_path.rs`
- All file APIs take a **relative** path string and resolve it with `SafeRelPath::resolve(root, input)`:
  1. reject absolute paths, drive letters, UNC prefixes, NUL bytes, `..` components after normalisation;
  2. join to `root` (the instance `server/` dir), canonicalise the deepest existing ancestor (`dunce` on Windows);
  3. verify the canonical path `starts_with(canonical_root)`. This catches **symlink and junction escapes**;
  4. Windows: reject reserved names (`CON`, `NUL`, `COM1`…) and trailing dots or spaces on create.
- Fuzz and property-test this module. It is the main security boundary.

## Operations
| Op | Notes |
|---|---|
| list | name, kind, size, mtime, permissions-ish; sort and filter client-side; hidden files toggle |
| read text | up to 5 MB (setting); binary detection (NUL bytes / invalid UTF-8 ratio) → "download instead" |
| write text | `expected_mtime` optimistic lock → `409 file_changed` if modified meanwhile; atomic write (temp + rename); keeps a `.gsd-prev` copy? (no: rely on backups; optional setting "keep last version") |
| upload | multipart streaming to a temp file then rename; chunked/resumable (P2) for >100 MB worlds; drag-and-drop folders (webkitdirectory) |
| download | file streaming; folders as an on-the-fly zip stream |
| mkdir / rename / move / copy / delete | delete goes to `<instance>/.gsd-trash/` (P1: restore from trash) or permanent with confirmation |
| extract | zip, tar.gz, tar.zst, with zip-slip protection (each entry through SafeRelPath) |
| compress | selection → zip |
| search | filename search (walkdir, limit); content grep (P2) |

## Running-server awareness
- Warn when editing files the server owns while running: `ops.json`, `whitelist.json`, `banned-*.json`,
  `usercache.json`, region files, `level.dat`, and suggest the Players UI instead.
- `server.properties` edits while running show "Restart required to apply" with a restart button.
- Jar changes in `mods/` or `plugins/` while running → "Restart required" banner.

## UI
- Two panes: tree (lazy-loaded) + table of the current folder with breadcrumbs; multi-select; context menu; keyboard shortcuts (F2 rename, Del, Ctrl+C/V for copy/move inside the app).
- Editor: CodeMirror 6 with modes by extension (json/json5, yaml, toml, properties, cfg/ini, mcfunction (simple
  highlighter), snbt, txt, log). Dirty indicator, Ctrl+S to save, diff view before save (P2), JSON/YAML/TOML
  validation on save with error location.
- Image preview (png, e.g. `server-icon.png`, with a "64×64 required" check), and a "set server icon" helper that resizes uploads.
