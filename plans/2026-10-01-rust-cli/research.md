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
