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
