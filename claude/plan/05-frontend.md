# Plan 05: Frontend (plain HTML + CSS + JS)

## Hard constraints (from the user)
- **Plain HTML, CSS and JavaScript.** No React, Vue or Svelte, and no TypeScript compile step. The user will later apply the
  **`/hallmark` design skill** to restyle it, so markup and styling must be cleanly separable:
  - semantic HTML with stable class names and `data-*` hooks for JS (JS never selects by styling classes);
  - **all visual decisions in CSS custom properties** (design tokens) in one file;
  - no inline styles from JS except dynamic values (for example the progress bar width set through a CSS var:
    `el.style.setProperty('--pct', 42)`);
  - component CSS files map 1:1 to components, so a redesign can swap them.
- No bundler required to run. Native ES modules (`<script type="module">`), import maps for vendored libs.

## Directory layout
```
web/
├─ index.html                 # app shell: <header>, <nav>, <main id="view">, <div id="toasts">, <dialog>s
├─ login.html · setup.html    # standalone pages (no app shell)
├─ css/
│  ├─ tokens.css              # colours, type scale, spacing, radii, shadows, motion; light + dark themes
│  ├─ base.css                # reset, typography, focus rings, forms
│  ├─ layout.css              # shell grid, sidebar, responsive breakpoints
│  └─ components/             # one file per component: button.css, card.css, tabs.css, console.css, …
├─ js/
│  ├─ main.js                 # boot: auth check → ws connect → router start
│  ├─ core/
│  │  ├─ api.js               # fetch wrapper (CSRF header, JSON, error → ApiError with .code)
│  │  ├─ ws.js                # single WebSocket, topic subscribe/unsubscribe, reconnect with backoff
│  │  ├─ router.js            # History API router: path patterns → view modules (lazy import())
│  │  ├─ store.js             # tiny observable store (subscribe/select/set), no magic
│  │  ├─ dom.js               # h()/html helpers, safe text insertion (never innerHTML with server data)
│  │  └─ format.js            # bytes, durations, relative time, MC colour codes → spans
│  ├─ components/             # Custom Elements (Web Components), light DOM (so global CSS applies)
│  │  ├─ gsd-console.js       # virtualised log view + input + autocomplete popup
│  │  ├─ gsd-version-picker.js# two-pane MC × loader picker
│  │  ├─ gsd-file-tree.js · gsd-code-editor.js · gsd-property-field.js · gsd-chart.js
│  │  ├─ gsd-job-tray.js · gsd-toast.js · gsd-confirm-dialog.js · gsd-player-head.js
│  └─ views/                  # one module per page; export mount(el, params) / unmount()
│     ├─ dashboard.js · instance-create.js · instance/*.js (overview, console, logs, players, content,
│     │  worlds, files, properties, schedules, backups, updates, settings)
│     └─ settings.js · java.js · jobs.js · audit.js
├─ vendor/                    # pinned, checked-in third-party ESM builds (see below)
└─ img/ · icons/sprite.svg
```

## Patterns
- **View lifecycle**: `mount(root, params)` renders, subscribes to the store and WS topics, and returns `unmount()` which
  unsubscribes everything. This prevents leaks when switching tabs.
- **Data flow**: REST for initial state → store; WS events patch the store; views re-render the affected parts.
  Keep it explicit: no virtual DOM. Components expose `update(data)` methods.
- **Security**: server-derived strings always go through `textContent` or `dom.text()`. Console lines with colour codes are
  parsed into spans built with `textContent`.
- **Progressive enhancement**: links are real `<a href>`; forms are real `<form>`s, enhanced with JS.
- **Accessibility**: keyboard navigation everywhere, visible focus, ARIA for tabs, dialogs and live regions (console
  uses `aria-live="off"` with a "screen reader: announce new lines" toggle), contrast checked in both themes.
- **Responsive**: sidebar collapses under 900px; console and file manager remain usable on a phone.

## Vendored libraries (minimal, pinned, ESM)
| Need | Choice | Why |
|---|---|---|
| Code editor (file manager, raw properties) | **CodeMirror 6** prebuilt to one ESM file by a one-off script in `tools/vendor-build/` (esbuild), committed to `web/vendor/` | Light, modular, good language modes (json, yaml, properties, toml); avoids Monaco's weight. The build tool is only for refreshing the vendor file, not the app. |
| Charts | **uPlot** (~50 KB) | Fast time-series, vanilla JS. |
| Console | **Custom** (not xterm.js) | The MC console is line-based, not a TTY. A custom virtualised list gives search, level filters, clickable player names and copy-friendly text. |
| Icons | Lucide SVG sprite | Consistent, tree-shakable by hand. |
| Markdown (notes) | tiny renderer, or plain textarea in v1 | |

## Page map and key screens
1. **Dashboard**: instance cards (icon, name, MC + loader badge, state pill, players x/y, CPU/RAM sparkline, quick
   start/stop/console). Grouping, search and sort. "New instance" CTA. Global job tray in the header.
2. **Create instance** (modal or page):
   - Step 1 "Software": `gsd-version-picker` copies the user's Prism screenshot: top list of MC versions (filters: Releases,
     Snapshots, Old beta, Old alpha; search; refresh), bottom list of loader versions with a loader radio group (Vanilla,
     Paper, Purpur, Fabric, Quilt, Forge, NeoForge); ⭐ for recommended or stable; "Released" date column. Unsupported
     combos are disabled with a tooltip.
   - Alternative tabs: "Modpack" (search Modrinth/CurseForge), "Import" (folder, zip, `.mrpack`).
   - Step 2 "Settings": name, icon, memory slider (with a host RAM hint), port (auto-picks a free one), Java (auto).
   - Step 3 "EULA": link to the Mojang EULA plus a required checkbox. Create → job progress → lands on Console.
3. **Instance page**, tabs: Overview · Console · Logs · Players · Mods/Plugins · Worlds · Files · Properties ·
   Schedules · Backups · Updates · Settings. A header bar is always visible: state, start/stop/restart, address to
   connect, players, CPU/RAM.
4. **Global**: Settings (general, API keys, network/remote access, notifications), Java runtimes, Jobs, Audit log.

## Hallmark hand-off notes
- Keep `tokens.css` the single source of colour, typography, spacing, radius, shadow and motion.
- Each component's CSS uses only tokens, never raw hex or px except hairlines.
- Class naming: `c-<component>`, `c-<component>__<part>`, `is-<state>` (`is-running`, `is-active`). Layout classes `l-*`.
- Do not bake visual polish into JS. When Hallmark runs, it should need to change only `css/` and maybe markup templates.
