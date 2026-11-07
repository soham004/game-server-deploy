# Plan 02: Rust Code Conventions (readable, maintainable, clean flow)

The user's hard requirement: **the backend must be readable and maintainable, with a clean logical flow.** These rules
apply to every PR. Future-me: re-read this before writing code.

## 1. Structure
- **Feature folders, not layer folders.** `backups/{model,service,archive,retention}.rs` beats `models/backup.rs` +
  `services/backup.rs` + …; everything about a feature sits together.
- **One responsibility per file**; aim for under 400 lines. Split by concept, not by size alone.
- **`mod.rs` / `lib.rs` only re-export and document.** No logic hiding in module roots.
- Every module starts with a `//!` doc comment of 2–5 lines: what it does and how it fits the flow.
- **Thin handlers → services → domain → infra** (see `01-architecture.md` §3). A handler never touches the filesystem,
  DB or process directly.

## 2. Functions
- Read top to bottom like a story: a public function at the top calls well-named private steps below it.
  ```rust
  pub async fn create_backup(&self, id: InstanceId, opts: BackupOptions) -> Result<BackupRecord, BackupError> {
      let instance = self.instances.get(id).await?;
      let _lock = self.locks.acquire(id, LockKind::Backup).await?;
      let quiesced = self.quiesce_world(&instance).await?;      // save-off + save-all flush
      let archive = self.write_archive(&instance, &opts).await; // always runs restore step after
      self.resume_world(quiesced).await?;                        // save-on
      let record = self.persist_record(&instance, archive?).await?;
      self.retention.apply(&instance).await?;
      Ok(record)
  }
  ```
- Prefer early returns over nested `if let`/`match` pyramids.
- No boolean parameters with unclear meaning: use an enum (`StopMode::Graceful | StopMode::Kill`).
- Keep functions under ~50 lines. If a comment says "step 3: …", that step probably wants its own function.

## 3. Types
- **Newtypes for IDs and validated values**: `InstanceId`, `JobId`, `McVersion`, `PropertyKey`, `SafeRelPath`.
  Parsing happens once at the edge (`TryFrom`/`FromStr`), then the type guarantees validity.
- Make invalid states unrepresentable: `enum ServerState { Stopped, Starting{since}, Running{since, pid}, Stopping{…}, Crashed{…} }`
  rather than a struct of `Option`s and flags.
- DTOs (`http/routes/*`) are **separate** from domain types. Map them explicitly. Domain types don't derive `Serialize`
  just to leak into the API, unless they are deliberately part of the wire contract (event payloads).
- `#[non_exhaustive]` on public enums that will grow (loaders, events).

## 4. Errors
- Each lib module has its own `thiserror` enum (`BackupError`, `InstallError`, …) with **actionable** variants:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum InstallError {
      #[error("Minecraft {mc} is not available for {loader}")]
      UnsupportedCombination { mc: McVersion, loader: LoaderKind },
      #[error("download failed: {url}")]
      Download { url: String, #[source] source: reqwest::Error },
      #[error("installer exited with code {code}; see job log")]
      InstallerFailed { code: i32 },
  }
  ```
- The `http/error.rs` layer converts service errors into `ApiError { status, code: &'static str, message, details }`.
  `code` values (`instance_not_found`, `server_running`, `unsupported_combination`, …) are a **stable contract** with the UI.
- `anyhow` is allowed only in `main.rs` and tests.
- **No `unwrap()`/`expect()` in non-test code**, except for provably infallible cases with a comment explaining why.
- Add context at boundaries (`.map_err(|e| InstallError::Download { url, source: e })`), not at every line.

## 5. Async and concurrency
- Never hold a `Mutex` guard across `.await`. Prefer the actor pattern (mpsc + owned state) for anything with a lifecycle.
- Use `tokio::sync::{Mutex,RwLock}` only for short critical sections; `std::sync::Mutex` is fine for non-async tiny state.
- CPU- or IO-heavy blocking work goes in `spawn_blocking`.
- Every long task takes a `CancellationToken` and checks it between steps.
- Spawned tasks are **owned**: store the `JoinHandle` or use a `JoinSet`; no fire-and-forget without logging failures.

## 6. External effects are injectable
- Services receive their dependencies through constructors (`BackupService::new(store, instances, supervisors, clock, fs_root)`).
- A `Clock` trait (or `chrono` injection) is used by the scheduler and retention, so tests can control time.
- HTTP-backed providers implement small traits (`trait VersionProvider`) so tests use fixture implementations.

## 7. Naming
- Modules and functions are verbs and nouns from the domain (`instances::start`, `backups::restore`, `mods::check_updates`).
- Avoid vague names: `utils.rs`, `helpers.rs`, `manager.rs` (except where "manager" is the real domain concept), `data`, `info`.
- Booleans read as questions: `is_running`, `has_eula`, `should_restart`.

## 8. Documentation
- Public items get `///` docs: what, not how. Note invariants and error cases.
- Non-obvious decisions get a `// WHY:` comment that links to `claude/decisions.md` when relevant.
- Each crate has a short `README.md`.

## 9. Lints and tooling (workspace `Cargo.toml`)
```toml
[workspace.lints.rust]
unsafe_code = "forbid"          # allow per-module only for OS FFI if a crate isn't available
missing_debug_implementations = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "warn"
expect_used = "warn"
module_name_repetitions = "allow"
missing_errors_doc = "allow"    # revisit later
```
- `rustfmt.toml`: defaults plus `imports_granularity = "Module"`, `group_imports = "StdExternalCrate"` (nightly-only
  options are fine to skip).
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo deny` (licenses, advisories).

## 10. Tests
- Unit tests next to code (`#[cfg(test)] mod tests`), fixtures in `crates/<c>/tests/fixtures/`.
- Golden tests for parsers: properties round-trip, log lines from Vanilla/Paper/Fabric/Forge/NeoForge, `commands.json`, `help` output.
- Integration tests use the **fake server** binary (`crates/gsd-app/tests/fake_server`) that prints MC-like logs and
  answers `stop`, `list`, `save-all`.
- Test names describe behaviour: `restores_save_on_even_when_archive_fails`.
