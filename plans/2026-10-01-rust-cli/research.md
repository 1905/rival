# Rust port implementation research

**Date:** 2026-10-02
**Status:** in-progress; findings below are not release validation.

## P1 dependencies

- YAML: `serde-saphyr` 1.3.0, released 2026-09-16. This meets the plan's release-within-12-months rule. The controller fetched the crate; config compatibility remains a test gate. [Release record](https://docs.rs/crate/serde-saphyr/1.3.0)
- ANSI cleanup: `strip-ansi-escapes` 0.2.1 uses the `vte` parser. It accepts bytes and returns stripped bytes. Match the Go progress-frame, CRLF and invalid-UTF8 rules around that parser. [API](https://docs.rs/strip-ansi-escapes/0.2.1/strip_ansi_escapes/)
- macOS process identity: local `libc` 0.2.189 exposes `proc_pidinfo`, `proc_bsdinfo`, `PROC_PIDTBSDINFO` and `pbi_start_tvsec/usec`. The latter match the units Go uses for PID identity. Live identity tests remain required.
- Follow-up evidence: `proc_pidinfo` cannot inspect PID 1 here, but Go's sysctl succeeds. The port now uses `sysctl kern.proc.pid`; Apple's installed `sys/sysctl.h` and `sys/proc.h` define the timeval prefix at offset zero. Self and launchd tests pass on this Mac. Linux native execution remains a CI gate.
- ANSI follow-up: strip-ansi-escapes drops tabs. Splitting around tabs breaks escape sequences containing them. The port now uses vte 0.14.1 directly and retains tabs through its execute callback. The original Go cases and the embedded-tab regression pass.
- `.env`: local source inspection found incompatible duplicate-key and interpolation behavior in `dotenvy` 0.15.7. `godotenv` 1.5.1 parses into a map first and expands variables from that map only. `dotenvy` consults process variables and has different token parsing. Port the existing Go parsing behavior and share it between startup and config credential lookup. This corrects the dependency assumption without changing the requested behavior. [Godotenv source](https://github.com/joho/godotenv/blob/v1.5.1/parser.go), [Dotenvy source](https://docs.rs/crate/dotenvy/0.15.7/source/src/parse.rs)

## Later platform work: researched, not implemented

- Queue locking: fd-lock 4.0.4 provides an exclusive guard released on drop and supports Windows. The crate is cached for P2; independent-process tests remain required. [API](https://docs.rs/fd-lock/4.0.4/fd_lock/struct.RwLock.html)
- Unix pipe cancellation: closing a descriptor from another thread does not reliably interrupt its blocking I/O on Linux. Rust must use cancellation-capable I/O to preserve Go's bounded drain; unsafe cross-thread close is insufficient. [Linux close(2)](https://man7.org/linux/man-pages/man2/close.2.html)
- Rust 1.98.1 still marks Windows `spawn_with_attributes` and `ProcThreadAttributeList` as nightly-only. P5 must not rely on those APIs under the pinned stable toolchain. [Rust process extensions](https://doc.rust-lang.org/std/os/windows/process/index.html), [CommandExt source](https://doc.rust-lang.org/stable/src/std/os/windows/process.rs.html)

- Microsoft documents Job Object kill-on-close semantics, including descendants. The provider owner must retain the sole controlling handle. Do not make that handle inheritable. [Job Objects](https://learn.microsoft.com/en-US/windows/win32/procthread/job-objects)
- `CreateProcess` suspended, assign to Job, then `ResumeThread` is the plan's required sequence. Microsoft also documents a crash window before assignment and an attribute-list alternative. Investigate that window when implementing P5; native process-tree tests remain required. [Microsoft discussion](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812)
- GoReleaser has a Rust builder using `cargo zigbuild`. Its default targets omit Windows ARM64. The six-target gate must therefore use an explicit target list and a verified Windows builder. No release approach is yet proven. [Rust builder](https://www.goreleaser.com/customization/builds/builders/rust/), [Actions guidance](https://www.goreleaser.com/blog/rust-zig/)
- GoReleaser accepts multiple Rust build entries with custom cargo command/flags, and maps Rust triples into the existing OS/architecture template values. Candidate: zigbuild for four Unix targets; `cargo xwin build` for two Windows MSVC targets. This is a documented route, not a tested build. [Rust builder configuration](https://goreleaser.com/customization/builds/builders/rust/)
- cargo-xwin supports Windows x86_64 and aarch64 SDK content and needs LLVM/Clang. It can run as a CI-installed tool without Docker on this Mac. [Upstream instructions](https://github.com/rust-cross/cargo-xwin/blob/main/README.md)

## Go release toolchain compatibility

- Releases use Go 1.25. The local Go 1.27.1 has JSONv2 enabled by default, which changes invalid-UTF8 encoding. Contracts and CI use Go 1.25.14, the current available patch listed by the official download API. The Rust writer preserves legacy Go byte escapes. [Go 1.25 notes](https://go.dev/doc/go1.25), [Go 1.27 notes](https://go.dev/doc/go1.27), [download records](https://go.dev/dl/?mode=json&include=all)
- Session durations use an in-memory monotonic clock while the recorded start time remains unchanged. Loaded sessions use wall time, as Go does. UTF-8 preview slices retain bytes until JSON serialization. Contract fixtures verify the exact writer output.

- Stable Windows spawn candidate: keep `std::process::Command`, start with `CREATE_SUSPENDED`, assign the child process handle to the Job, then resume its thread through Toolhelp32. Watchexec uses this sequence. Its std wrapper does not enable kill-on-close by default, so Rival must set that policy explicitly. This avoids custom `.cmd` quoting and is not yet tested here. [Upstream implementation](https://github.com/watchexec/process-wrap/blob/main/src/windows.rs), [std wrapper](https://github.com/watchexec/process-wrap/blob/main/src/std/job_object.rs)
- Windows synchronous pipe cancellation needs a thread handle with `THREAD_TERMINATE` access. Cancellation can race with the next I/O call, so workers also need a stop check. A single cancellation call alone does not prove bounded return. [Microsoft API](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelsynchronousio)

- GoReleaser constructs the Rust command as `tool command --target=<triple> flags...`. For Windows, use `tool: cargo-xwin` and `command: build`; putting `build` in flags puts it after `--target`. cargo-xwin explicitly supports direct `cargo-xwin build` invocation. The six-target snapshot still needs to prove this configuration. [GoReleaser source](https://github.com/goreleaser/goreleaser/blob/main/internal/builders/rust/build.go), [cargo-xwin entry point](https://github.com/rust-cross/cargo-xwin/blob/main/src/bin/cargo-xwin.rs)

## P1 CI findings

- Hosted macOS and Linux passed Rust checks. Existing Go tests assumed installed OpenCode and used fake processes that exited before consuming stdin. Test-only fixtures now remove those dependencies; the full Go suite passes with provider CLIs excluded from PATH.
- Hosted Git prints UTC as `Z` where local Git prints `+00:00`. The fixture assertion now checks both author/committer names and exact epoch timestamps.
- Hosted macOS signal-test startup failed before readiness. The first failure lacked child diagnostics; its cause is not established. CPython recommends an absolute executable path and `sys.executable` for restarting Python, which the test already uses. Add diagnostics before selecting a fix. [CPython subprocess guidance](https://github.com/python/cpython/blob/main/Doc/library/subprocess.rst)

- The runner now avoids `HTTPServer.server_bind`'s reverse DNS lookup. It binds directly to `127.0.0.1` and assigns the local server name. Timeout diagnostics retain runner/step output and request stacks only after registration. Test cleanup matches unique task-root environment markers. These changes do not yet prove the cause of the hosted macOS timeout.

- P1 follow-up: CI 37011767761 passed on macOS and Linux at `50e3eea`, including all 55 Python tests and required Swift decoding. The startup timeout did not recur. This proves the revised suite passed; it does not establish DNS as the original cause.

## P2 source checks

- Cached `fd-lock` 4.0.4 source (`src/sys/unix/mod.rs`) uses `rustix::fs::flock` on macOS/Linux. This matches the Go queue's lock mechanism. Three independent helper processes pass the FIFO and mutual-exclusion check locally and in P2a CI 37018189761 on macOS/Linux.

### Detach standard streams

- Rust's Unix startup reopens closed standard descriptors on `/dev/null`. The original state is gone before `main`, so a caller that closed stderr is a compatibility gap. Normal inherited files and pipes are covered by the detach unit test. Closed-before-startup behavior remains unresolved for the P3 command gate; do not claim exact parity for it. [Rust runtime source](https://doc.rust-lang.org/src/std/sys/pal/unix/mod.rs.html)
- Go normally exits on SIGPIPE when writing to a broken stdout/stderr pipe. Rust ignores SIGPIPE and returns a write error. The approved task explicitly requires killing the new detached child on a failed PID notice; the Rust implementation follows that requirement. Its failing-writer test passes. A real broken-pipe command check remains for P3. [Go signal behavior](https://pkg.go.dev/os/signal#hdr-SIGPIPE), [Rust runtime source](https://doc.rust-lang.org/src/std/sys/pal/unix/mod.rs.html)
- The redirected-input scenario unlinks stdin after the parent exits. It cannot force the actual detached child's first read to occur after the unlink. File-descriptor identity and the runner's controlled detached reader cover the mechanism; actual command ordering remains a stated test limit.

### Executable format errors

- Local Task2.4 tests exposed a macOS difference: Rust `Command::spawn` ran an executable text file without a shebang through `/bin/sh`. Both the preflight and provider runner returned success and wrote the test marker. Go's direct `execve` rejects that file.
- Apple's `posix_spawnp` source explicitly retries `ENOEXEC` with `/bin/sh`, including paths containing `/`. A filename lookup change cannot remove that fallback. [Apple libc source](https://raw.githubusercontent.com/apple-oss-distributions/Libc/main/sys/posix_spawn.c)
- `execve` reports `ENOEXEC` for an unrecognized file format. It accepts explicit argument and environment arrays. [Apple execve documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/execve.2.html)
- Selected correction, awaiting tests: retain Rust `Command` and register a narrow `pre_exec` hook that calls `execve`. Prepare all strings and pointer arrays before the fork. The hook must not allocate, lock or format. Rust documents that stdio and cwd are already set, and a hook's OS error reaches the parent. The cached 1.98.1 source also sets the process group before the hook, but applies the command environment afterward. The hook must therefore pass its own prepared environment. [Rust CommandExt documentation](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec)
- Both required regression tests must run without an ignore attribute. Existing normal-execution, argument, environment, cancellation and pipe-drain tests must still pass. No file-header heuristic or separate custom process launcher is planned.
- Local follow-up: `executor::process::set_exec` now prepares the owned strings and pointers, then calls `execve` from the hook. Both format-error tests pass without skips. Controller verification passed 360 workspace tests, formatting and Clippy. Environment order and raw bytes are preserved too. Hosted verification remains pending. Unix launches now use Rust's fork path; the effect on launch time is unmeasured.
- Hosted follow-up: CI 37026571060 passed on macOS and Linux at `8e10ad1`, including the new executable-format, argument and environment-byte checks. The process-group and drain tests also passed through the changed launch path.

### Parser and Git scope

- Task2.5 preserves the Go parser's literal-space `-re` grammar, exact option errors, simple Unicode case mapping and original-byte review slicing. Source-derived tests cover the unusual `revİew` case and `-h=x` help behavior.
- Git scope uses the shared direct execution helper. Tests confirm argv, logical PWD and inherited repository overrides with a temporary fake Git executable. Real Git tests use temporary repositories and homes.
- Git stdout currently uses lossy UTF-8 decoding. Go retains raw bytes. Invalid-byte filenames with `core.quotepath=false` can therefore differ; default quoted output is covered. Windows PWD handling remains assigned to P5.
- Controller verification passed 407 workspace tests, formatting and Clippy. CI 37028459182 passed on macOS and Linux at `6030678`, including Swift session decoding.

### Review contracts

- Task2.6 compares all six review prompt pieces against Go source text, then retains fixed length/SHA-256 pins for P6. Both assembled default prompts are pinned too. A separate Python source evaluator confirmed all eight hashes without running Go.
- JSON key presence is case-sensitive; struct-field decoding uses Go case folding. Tests preserve duplicate-key order, slice reuse, null handling, huge unknown numbers and first type-error text. The final-answer header requires the exact line `codex`, without CR.
- Go nil and empty finding slices become an empty Rust vector. No command serializes that distinction. Provider-log APIs accept UTF-8 strings; invalid-byte handling remains a boundary limitation for the command port.
- SlotRelease frees an acquired ticket on every return path. Tests verify partial mark-running rollback, cancellation, timeout and queue-unavailable fallback with temporary paths and injected stderr.
- Controller verification passed 490 workspace tests, formatting and Clippy. CI 37030778213 passed on macOS and Linux at `b8f9bab`, including Swift decoding. Runtime use of final-answer helpers remains Task2.7/P3.

### MR URL parsing and snapshots

- Go preserves host spelling and explicit ports, lowercases schemes, and does not resolve path dot segments during parsing. A bare trailing query marker survives Rival's clearing of RawQuery through ForceQuery. Task2.8 must test these differences before selecting a URL helper. [Go 1.25.14 URL source](https://raw.githubusercontent.com/golang/go/go1.25.14/src/net/url/url.go)
- Task2.8 now covers those rules in a private MR URL helper. An initial general URL implementation was reduced to the operations MR validation uses. Host/project bytes remain exact for identity checks; invalid UTF-8 in displayed errors or provider prompts remains lossy.
- Real local Git fixtures prove exact base/head checkout, the 512 KiB patch limit, disabled hooks and unchanged caller files/index/refs. Test Git processes deny network protocols. Fake glab checks host-specific authentication and rejected stale/mismatched responses.
- Cancellation during fetch removes an already-created snapshot. Close tests remove regular files and symlinks while preserving outside targets. Scoped cancellation also runs during unwinding. The Go child-only kill and unbounded pipe-EOF behavior are preserved; provider process-group cleanup is separate.
- Controller verification passed 570 workspace tests, formatting and Clippy. All 45 scenario schemas validate; command execution remains P3. Malformed MR JSON uses serde_json's error detail. Removal failures name the snapshot path rather than Go's failing child entry. Hosted verification is pending.

### Concurrent review runs

- Task2.7 starts all selected reviewers before joining in requested order. Bounded fake reviewers prove concurrent execution, stable results, per-model efforts and a single queue ticket for the whole batch.
- Two controller findings were corrected: unfinished-session finalization and run-context cancellation now also execute during unwinding. Panic tests reload the failed session and verify that the parent context remains live.
- Controller verification passed 532 workspace tests, formatting and Clippy. CI 37033031072 passed on macOS and Linux at `86c3ee7`, including Swift decoding. Thirty-eight scenario schemas validate; command execution remains pending P3. Plan and document runtime parsing now uses the final-answer helper. Code-review/security command paths remain P3.

### Isolated MR transport fixture

- `git remote get-url` expands URL rewriting, so an unconditional fixture rewrite would break remote identity validation. A conditional include limits rewriting to `**/tmp/rival-mr-*/`. [Git remote documentation](https://git-scm.com/docs/git-remote.html), [Git conditional includes](https://git-scm.com/docs/git-config/2.44.3.html)
- A local probe verified unchanged caller HTTPS identity, snapshot-only local rewriting and disabled network protocols. Absolute patterns through macOS `/var` symlinks failed; the narrow directory pattern passed. The full MR scenario remains unrun until P3.
## Runner diagnostic interleaving — 2026-10-03

CI 37037313215 passed the Rust checks but failed one Linux runner self-test. Two fake processes dumped stacks into the same step stderr file. Their writes interleaved, splitting the expected `in act` frame text. The failure output directly shows both stacks combined. This is a diagnostic fixture failure, not an observed Rust runtime failure.

The narrow repair gives each registered fake a task-owned stack file. The readiness report collects each separately. Assertions still require a useful stack from every signalled fake and verify owned-process cleanup. All 58 runner tests passed locally. CI 37038456086 passed on macOS and Linux for `9f6f562`.
## Scoped signal cancellation — 2026-10-03

Cached `ctrlc` 3.5.2 source shows one permanent handler; its `termination` feature also intercepts SIGHUP. Cached `signal-hook-registry` 1.4.8 documents that unregistering the last action does not restore the previous/default handler. Neither is a direct match for Go's scoped SIGINT/SIGTERM subscription. `signal-hook` 0.3.18 remains an available implementation option where its semantics fit.

[POSIX signal actions](https://man7.org/linux/man-pages/man2/sigaction.2.html) can save and restore the previous disposition. If the port uses that interface, pipe/thread/flag/action setup must handle errors. Signal-delivery tests must use isolated helpers so they cannot cancel other concurrent unit tests. The implementation and native-host proof are still in progress.

## Root and model commands — 2026-10-03

The command tree uses clap for parsing, with narrow adapters for Rival's existing validation errors and values. The initial duplicate Cobra parser was replaced before acceptance. Config loading preserves Go's pre-dotenv user config and wait default while refreshing both runtime environment representations afterward.

SIGINT/SIGTERM use a scoped self-pipe handler that restores prior dispositions. Pipe, fcntl, thread and sigaction setup errors propagate. Isolated child tests prove real signal cancellation, restoration and recovery after an actual descriptor-limit setup failure.

The macOS loader constructor records closed standard descriptors before Rust sanitizes them. Controller checks passed for debug and LTO release binaries: closed stdin returns the expected read error; closed or broken stderr exits 1 without leaving a task-owned detached child. Stdin stayed open during the child-liveness check. CI 37044718890 passed the linked debug/release checks on both macOS and Linux at `3023181`, including the existing Swift decode contract.

Controller checks: 669 workspace tests, formatting, Clippy and 11 fake-provider/detach/wait scenarios passed. One older subprocess fixture failed because it published a PID before echoing its required output. It now emits the output first; its timeout and manual-cancel assertions remain unchanged. Full P3 scenarios and real-review checks remain pending.

## Telemetry port checks — 2026-10-03

The cached Sentry0.49.3 Cargo.toml.orig defines `ureq` and `rustls` as separate features. Disable defaults and request both to avoid the default reqwest/native-tls stack. The built-in ureq transport sends through a standard background thread; it is not the Go synchronous transport. Use the SDK transport and its bounded flush rather than inventing a second sender. [Sentry transport documentation](https://docs.rs/sentry/0.49.3/sentry/transports/index.html)

Go's `cmd.Execute` defers `telemetry.RecoverPanic`, which calls `sentry.Recover`, which calls `recover`. That extra wrapper prevents recovery: Go requires the recover call directly in the deferred function. The cached sentry-go0.43.0 source confirms this call chain. This is a source-derived bug, not a live panic measurement. Preserve the port's stated no-silent-bug-fix rule and record this exception before choosing panic capture behavior. [Go recovery specification](https://go.dev/ref/spec#Handling_panics)

## Plan, antislop and security commands — 2026-10-03

The three commands now use the existing review orchestration and final-answer helpers. Tests preserve native/stdin model and effort conflicts, path spacing and escaping, preflight order, scope fallback and both output streams. Security's failed stdin stat skips the read; model/plan/antislop retain their read-error behavior.

Security completion-save errors return exit 1. If the session directory also prevents the failure save, its stored session remains running, as in Go. The test checks this limit instead of claiming successful persistence. Windows home/path/stat behavior remains assigned to P5.

Controller verification passed 734 workspace tests, formatting, Clippy, build and all 12 plan/antislop/security scenarios. All 53 scenario schemas validate. The escaped-pipe fixture had the same unguarded marker order as the earlier launcher fixture. Its marker now precedes helper startup; bounded draining, descriptor forwarding and identity checks are unchanged. CI 37046983286 passed on macOS/Linux at `daa123a`, including Swift decoding.

## Embedded skills and installation — 2026-10-03

All nine skill files and the Codex workflow template match the Go assets byte for byte. Generated variants retain Go's descriptions, input rules and command substitutions. Installation uses the user home independently of `RIVAL_HOME`. One buffered reader serves both targets.

Controller checks passed: 774 workspace tests, formatting, Clippy, four installer scenarios and 57 scenario schemas. The version-bump script updated all 18 skill copies in a temporary repository. The 20 original asset files stayed unchanged. Missing skill files now make that maintenance script exit nonzero.

Source quirks remain visible: the Codex antislop instructions name a single default reviewer, dangling deprecated symlinks survive cleanup, and a partial hash-cleanup failure loses its removal count. Go's Windows embedded-file lookup also uses backslashes against `embed.FS`; Rust keeps slash asset paths so the approved Windows install can work. Native Windows validation remains P5. Failed-parent path detail and invalid UTF-8 remain the recorded filesystem boundary limits.

## Unpublished release dispatch — 2026-10-03

The release workflow already exists on master and has prior runs. Keep its path when adding snapshot dispatch. GitHub documents branch dispatch through CLI/API for an already-run workflow. The intended P6 check uses the feature branch, without a release tag or an early Rust-switch merge. This dispatch has not yet been tried with the revised workflow. [GitHub dispatch documentation](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch)

## Remaining commands and P3 checks — 2026-10-03

Queue/session output, the version banner, update client/cache, Homebrew update and telemetry lifecycle now have Rust implementations. The client decodes the first HTTP JSON value without waiting for EOF and samples cache time after a successful fetch. Homebrew receives null stdin; the upgraded installer receives the caller's input. Scoped test writers join before return.

The controller verified 815 workspace tests, formatting, Clippy, build, 67 runner tests and the two later raw-hash tests. The implementer also ran all 69 runner tests together. Nineteen release-profile update tests passed; the controller independently reran the compiled release endpoint-override test. All 84 CLI scenarios passed across the full run and three corrected fixtures. The corrections preserve Go's high-to-max Claude effort mapping, read-only review arguments and unchanged stale-cache bytes. CI 37053147082 passed all 84 scenarios on macOS and Linux. P3 merged at `bb95462`; post-merge CI 37054358089 also passed.

An actual detached fake review held one running queue ticket. Killing its owner made `wait` return 3 while the provider remained alive. After task-owned provider cleanup, normal `sessions` startup marked the orphan failed with exit 1 and removed the queue ticket. No task processes survived.

A real Codex review through the Rust binary completed in 208.45 seconds. Rival.app displayed the completed session and parsed finding from the private test home. The finding concerns interrupted skill writes: Go uses `os.WriteFile`, which truncates before writing. A partial write can preserve a new version header and defeat the next version-only check. This is an existing Go risk, retained under the approved port contract. The live plan review completed in 682.32 seconds and also rendered correctly in Rival.app. Both commands exited 0. The task credential copy was removed and the test app closed; original account state was unchanged.

Compatibility limits: update transport and JSON syntax diagnostics use ureq/serde wording. ureq uses its User-Agent, webpki roots and proxy rules, without Go's gzip behavior. The inherited child-stdio update path passes Rust's null-device replacement if a descriptor was closed before startup. These limits are separate from the verified detach closed-descriptor contract. Sentry uses one SDK transport thread with a two-second flush; command errors bypass that flush as Go's `os.Exit` does. No automatic error or panic event is added. Interactive TUI acceptance remains P4; native Windows checks remain P5.

## Windows containment and console corrections — 2026-10-03

The required live plan test found three future implementation gaps. Full output is in `reviews/p3-live-plan-review.txt`. Plan v2.3 records the corrections within the approved scope. Native Windows implementation and verification remain pending P5.

Creating a suspended provider and assigning it to a Job afterward leaves an owner-death window. Microsoft documents this exact orphan risk and an atomic job-list attribute. Rust's stable `Command` does not yet expose that attribute; `spawn_with_attributes` remains nightly. [Microsoft explanation](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812/), [Rust CommandExt](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html).

The selected stable mechanism puts the review owner inside one unnamed, non-inheritable, kill-on-close Job before any provider spawn. Children inherit that containment at creation. Cargo uses this owner-Job pattern. Rival must treat setup errors as fatal before spawning; it must keep the handle alive for the whole process. The detach launcher must not join this Job before re-exec. [Cargo implementation](https://doc.rust-lang.org/nightly/nightly-rustc/src/cargo/util/job.rs.html), [Job inheritance](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).

Each provider still gets a nested Job for independent cancellation. It starts suspended, inherits the owner Job, joins its nested Job, then resumes. Owner death before nested assignment is therefore contained. Native tests must kill the owner at that exact barrier. Other required checks cover concurrent providers, normal exit and nested Jobs under CI. This is a documented design, not a verified fix yet. [Nested Jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs).

`DETACHED_PROCESS` removes the console connection. Inherited console handles cannot provide the promised stream behavior without a console. The selected detach flags preserve the existing console whenever any standard handle is console-backed. Fully redirected streams can also use `DETACHED_PROCESS`. `CREATE_NO_WINDOW` does not repair this and is ignored beside `DETACHED_PROCESS`. Native tests must cover console, mixed and fully redirected handles after the launcher exits. Closing the console itself removes that I/O resource. [Console creation](https://learn.microsoft.com/en-us/windows/console/creation-of-a-console), [Console handles](https://learn.microsoft.com/en-us/windows/console/console-handles), [Creation flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags).

The third finding concerns Go retirement. P6 must remove source-reading checks and Go CI steps while retaining prompt hashes and fixture contracts. Full Rust and runner suites must pass again after removal. A pre-removal green run does not satisfy that gate.

## TUI session data — 2026-10-03

The cache reuses unchanged summaries by file size and modification time. Group reducers preserve status precedence, member ordering and whole-group elapsed time. The watcher sends the first snapshot and then watches JSON/log changes. Its owned guard cancels and joins the worker, including a blocked send. Initial progress never blocks.

Controller validation passed 851 workspace tests, formatting and Clippy. CI 37056093124 passed on macOS/Linux at `3856094`, including all 84 CLI scenarios and Swift decoding. The new tests cover the 17 Go group/cache cases, the loader case, real file notifications, cancelled/full-channel sends and shutdown.

The macOS notify backend is FSEvents, whereas Go uses kqueue. A rename away can therefore trigger an extra refresh. Watch setup errors use notify's detail. Equal-start-time summaries retain filename order; Go's map iteration leaves ties unspecified. These are TUI data differences, with no session-file format change. Interactive acceptance remains pending.

## VHS terminal setup — 2026-10-03

The first Go baseline recording stayed blank. Waiting for the actual empty-state text also failed. A local PTY probe under RTK 0.48.0 returned `isatty = [true, false, false]`: even `rtk proxy` gives its child output pipes. Its passthrough runner uses the streaming layer. [RTK runner source](https://raw.githubusercontent.com/rtk-ai/rtk/v0.48.0/src/core/runner.rs).

The task-owned capture helper now reattaches stdout/stderr to its VHS `/dev/tty` before executing Rival. A native empty-state screenshot then passed and showed the installed Go v4.1.1 TUI. The recorder uses a private HOME and disables telemetry/update checks. Its dummy process was reaped; a cwd-scoped process check found no surviving Rival/VHS/browser processes. Full Go/Rust fixture comparison remains pending P4.

## TUI model and geometry — 2026-10-03

The model routes keys by list, filter, detail, search and confirmation modes. Text input cannot trigger list shortcuts; Ctrl+C exits from every mode. Layout retains the 60×16 minimum, preview from 120 columns, 50-run pages and compact header below 30 rows. The model takes injected events and a clock; it does not perform file or terminal I/O.

Controller checks passed 915 workspace tests, formatting and Clippy. The 64 new tests cover the Go key/layout/loader cases, input routing, resize bounds, timer ownership and styles. A draft clipping bug split joined emoji. Grapheme-based clipping now has regressions for emoji, flags, combining marks and CJK text. Line-input editing still uses characters, like Go's rune-based input; a long input can start its visible window inside a cluster.

Ratatui uses the crossterm backend. Its lockfile also lists optional backends; earlier dependency versions remain intact. The logo gradient uses the app's linear RGB blend. Colors use RGB without Go's terminal-profile downsampling. Full list/detail content and terminal cleanup remain in later P4 tasks; no interactive Rust acceptance is claimed here.
