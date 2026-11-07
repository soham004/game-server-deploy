# Feature: Background Jobs

Everything slow is a Job: installs, imports, Java downloads, backups, restores, mod installs and updates, update checks,
command-tree generation, clone.

```rust
pub struct JobSpec { kind: JobKind, instance: Option<InstanceId>, title: String, locks: Vec<LockKind> }
pub trait JobTask: Send + 'static {
    async fn run(self: Box<Self>, ctx: JobCtx) -> Result<JobOutput, JobError>;
}
pub struct JobCtx { id: JobId, cancel: CancellationToken, progress: ProgressReporter, log: JobLog }
// progress.step("Downloading libraries").pct(42) → Event::JobProgress (throttled to ~5/s)
```
- `JobRunner::submit(spec, task) -> JobId`. Acquires instance locks (`Exclusive` for install/restore/version change;
  `Shared` for backup or mod check) before running, and fails fast with `instance_busy` otherwise.
- Concurrency limits: a global semaphore for downloads (4) and CPU-heavy compression (1–2).
- Cancellation is cooperative: tasks check `ctx.cancel` between steps and clean up partial files (temp dir + rename on success).
- Finished jobs are persisted to the `jobs` table (status, error, truncated log) for the Jobs page.
- UI: a header **job tray** with a spinner count; a dropdown shows progress bars, cancel buttons and "view log".
