# Feature: Process Supervisor and Console

## Supervisor actor (gsd-core/supervisor)
```
SupervisorHandle (Clone) ──mpsc<SupervisorCmd>──► SupervisorActor (one tokio task per instance)
                                                    owns: Option<Child>, stdin writer, state, ring buffer
                                                    spawns: stdout reader task, stderr reader task, exit waiter
                                                    publishes: EventBus (StateChanged, ConsoleLine, …)
```
Commands:
```rust
enum SupervisorCmd {
    Start { reply: oneshot::Sender<Result<(), StartError>> },
    Stop  { mode: StopMode, reason: StopReason, reply: … },   // Graceful(timeout) | Kill
    Restart { reason, reply },
    SendLine { line: String, source: CommandSource, reply },  // user / scheduler / system
    Snapshot { reply: oneshot::Sender<SupervisorSnapshot> },  // state + last N lines
}
```

### State machine
```
Stopped ──Start──► Starting ──"Done (x)!" or MCSMP server/started──► Running
Starting/Running ──Stop──► Stopping ──exit──► Stopped
Starting/Running ──unexpected exit──► Crashed ──(auto_restart & under crash-loop limit)──► Starting (after backoff 5s,15s,45s)
Crashed ──crash-loop limit hit──► Stopped{exit} + Diagnostic("crash loop") + notification
Stopping ──timeout──► kill ──► Stopped
```
- Transitions live in one pure function `fn transition(state, input) -> (ServerState, Vec<Effect>)`, unit-tested
  without processes. The actor executes the `Effect`s (spawn, write stdin, kill, schedule restart, publish).

### Starting a process
1. Resolve Java (instance setting → managed runtime → error with a fix button).
2. Build the command from the `LaunchSpec`: `java <memory> <preset flags> <extra> -Dfile.encoding=UTF-8 [-jar X | @user_jvm_args.txt @…args.txt] nogui`.
   For args-files launches, memory flags are passed on the command line (not by editing `user_jvm_args.txt`, which we leave alone).
3. `cwd = instance/server`, stdin/stdout/stderr piped, env cleaned (`JAVA_TOOL_OPTIONS` removed unless the user set it).
4. **Windows**: assign the child to a Job Object with `KILL_ON_JOB_CLOSE`, so killing the app never orphans servers, and kill = terminate the job.
   **Unix**: `process_group(0)`; kill = `SIGTERM` the group, then `SIGKILL` after a timeout.
5. Pre-flight checks before spawn: port free, EULA accepted, Java major ≥ required, disk not full.

### Graceful stop
`stop\n` to stdin → wait for exit up to `stop_timeout_secs` (default 60, configurable; large modded servers need more)
→ else kill and warn ("server did not stop in time; world may not be saved").

### When the app dies
On Linux every server gets SIGTERM from the kernel if the app dies without stopping it, and Java's shutdown hook saves the
world (D-047). A new app doesn't adopt or kill leftovers; if one still holds the world, the start fails with the
`world_locked` diagnostic.

### Output handling
- Reader tasks use `BufReader::lines()` on stdout and stderr (lossy UTF-8), parse each line with the game adapter
  (`LogLine{ts, thread, level, logger, text, raw}`), push it into a **ring buffer** (default 5,000 lines, configurable),
  publish `ConsoleLine`, and run **event detection** (ready, join/leave, save complete, diagnostics patterns).
- Backpressure: the broadcast channel drops for lagging WS clients (they resync from the snapshot); the ring buffer never blocks the reader.
- The server itself writes `logs/latest.log`, so we don't duplicate disk logging.

## Console UI (`gsd-console`)
- Virtualised list (renders only visible rows) to handle 10k+ lines smoothly; "follow tail" auto-scroll that pauses when the user scrolls up, with a "Jump to latest (N new)" pill.
- Level colouring (INFO/WARN/ERROR), `§`/ANSI colour support, clickable player names (opens the player drawer), clickable URLs.
- Filters: level checkboxes, text search with highlight, "hide noisy" (regex list in settings).
- Input: single line, history (↑/↓, persisted per instance in localStorage), `Tab`/`Ctrl+Space` autocomplete (see
  `command-autocomplete.md`), optional leading `/` stripped. Commands sent by the user are echoed as `> cmd` with the user's name (audit).
- Toolbar: start/stop/restart/kill, copy selection, download the visible buffer, clear view (client-only).
- Disabled input with an explanation when the server is stopped ("Start the server to send commands").

## Diagnostics (gsd-minecraft/diagnostics.rs)
Table-driven: `(Regex, DiagnosticCode, severity, title, explanation, fix: Option<FixAction>)`.
Initial set: wrong Java version, port in use, EULA, OOM (`java.lang.OutOfMemoryError`), missing mod dependency
(Fabric "requires mod X", Forge "Missing or unsupported mandatory dependencies"), client-only mod on the server
(`Environment type CLIENT` / `NoClassDefFoundError: net/minecraft/client`), mixin failure (show mod id), corrupted
chunk, `Can't keep up!` frequency (performance hint), crash-loop.
