package cmd

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/procinfo"
	"github.com/spf13/cobra"
)

// rival wait exit codes — distinct so a watcher / script can branch on outcome.
const (
	waitExitCompleted = 0 // all watched sessions completed
	waitExitFailed    = 2 // at least one session failed (incl. run timeout)
	waitExitCrashed   = 3 // rival died but left a session non-terminal
	waitExitTimeout   = 4 // --timeout elapsed while still running
	waitExitRunning   = 5 // --heartbeat elapsed; still running, re-arm the watcher
	waitExitUsage     = 64
)

var (
	// "rival: detached pid=12345" (from cmd/detach.go).
	detachedPIDRe = regexp.MustCompile(`rival: detached pid=(\d+)`)
	// zerolog: ..."session":"<uuid>"...
	sessionIDRe = regexp.MustCompile(`"session":"([0-9a-fA-F-]{36})"`)
	// Only THIS run's start lines (message:"starting codex|…"); excludes
	// reaper/maintenance lines that carry old session IDs.
	startingMarkerRe = regexp.MustCompile(`"message":"starting `)
	// A bare UUID — used to validate user-supplied positional IDs so they can
	// never escape the session dir via path separators (`rival wait ../x`).
	sessionIDOnlyRe = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)
)

func isSessionID(s string) bool { return sessionIDOnlyRe.MatchString(s) }

var waitCmd = &cobra.Command{
	Use:   "wait [session-id...]",
	Short: "Block until review session(s) finish; exit code reflects the outcome",
	Long: `Wait for one or more rival review sessions to reach a terminal state.

Two modes:

  rival wait --log <stderr-file>   (used by skills)
      Parse the detached rival PID and session IDs from a run's stderr file,
      poll the rival process for liveness, then summarize the sessions when it
      exits. Detects a crashed rival (process dead, sessions not finalized).

  rival wait <session-id>...       (terminal-status only)
      Poll the named sessions' JSON until all reach a terminal state.
      Note: a session is marked terminal moments before its output is flushed
      to the launching command's stdout; prefer --log when that matters.

  --heartbeat <d> returns early with exit 5 while the run is still going, so a
  background watcher wakes its caller to report progress and then re-arms.

Exit codes: 0 all completed · 2 some failed · 3 rival crashed · 4 timed out · 5 heartbeat.`,
	RunE: waitAction,
}

func init() {
	waitCmd.Flags().String("log", "", "stderr file of a detached run to parse pid + session IDs from")
	// Default derived from the queue + run timeouts so it can't expire while a
	// run is still within its configured budget (see config.MaxRunWait).
	waitCmd.Flags().Duration("timeout", config.MaxRunWait(), "give up waiting after this long")
	waitCmd.Flags().Duration("poll", config.QueuePollInterval, "poll interval")
	waitCmd.Flags().Duration("heartbeat", 0, "exit 5 after this long if still running, so a watcher can report progress and re-arm (0 = off)")
	rootCmd.AddCommand(waitCmd)
}

func waitAction(cmd *cobra.Command, args []string) error {
	logFile, _ := cmd.Flags().GetString("log")
	timeout, _ := cmd.Flags().GetDuration("timeout")
	poll, _ := cmd.Flags().GetDuration("poll")
	heartbeat, _ := cmd.Flags().GetDuration("heartbeat")
	if poll <= 0 {
		return &ExitCodeError{Code: waitExitUsage, Err: fmt.Errorf("--poll must be > 0")}
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	w := &waiter{
		poll:        poll,
		timeout:     timeout,
		heartbeat:   heartbeat,
		loadSession: loadSessionStatus,
		ralive:      procinfo.Alive,
		now:         time.Now,
		out:         os.Stdout,
	}

	if logFile != "" {
		if len(args) > 0 {
			return &ExitCodeError{Code: waitExitUsage, Err: fmt.Errorf("pass either --log or session IDs, not both")}
		}
		pid, pidStart, ids, err := parseLogFile(logFile)
		if err != nil {
			return &ExitCodeError{Code: waitExitUsage, Err: err}
		}
		// IDs from --log are regex-matched UUIDs already; positional IDs below
		// are user input and must be validated before becoming a file path.
		w.pid, w.pidStart, w.ids, w.logFile = pid, pidStart, ids, logFile
	} else {
		if len(args) == 0 {
			return &ExitCodeError{Code: waitExitUsage, Err: fmt.Errorf("provide --log <file> or one or more session IDs")}
		}
		for _, id := range args {
			if !isSessionID(id) {
				return &ExitCodeError{Code: waitExitUsage, Err: fmt.Errorf("invalid session ID %q (expected a UUID)", id)}
			}
		}
		w.ids = args
	}

	code := w.run(ctx)
	if code == waitExitCompleted {
		return nil
	}
	return &ExitCodeError{Code: code, Err: fmt.Errorf("rival wait: exit %d", code)}
}

// sessionStatus is the minimal slice of a session JSON wait needs.
type sessionStatus struct {
	ID       string
	Status   string
	ExitCode *int
	Duration string
	ErrorMsg string
	found    bool
}

func (s sessionStatus) terminal() bool {
	return s.Status == "completed" || s.Status == "failed"
}

// waiter holds the poll loop's state; deps are injectable for tests.
type waiter struct {
	pid      int
	pidStart int64
	ids      []string
	logFile  string

	poll      time.Duration
	timeout   time.Duration
	heartbeat time.Duration // 0 = off

	loadSession func(id string) sessionStatus
	ralive      func(pid int, start int64) bool
	now         func() time.Time
	out         io.Writer
}

// run polls until an outcome is decided and returns the exit code. It prints a
// one-line summary per session (or a crash/timeout line) before returning.
func (w *waiter) run(ctx context.Context) int {
	start := w.now()
	deadline := start.Add(w.timeout)
	havePID := w.pid > 0
	firstPoll := true

	for {
		// In --log mode, re-scan the file each tick: a run logs its "starting"
		// session lines only after it leaves the queue, so the IDs can appear
		// after wait has started.
		if w.logFile != "" {
			if _, _, ids, err := parseLogFile(w.logFile); err == nil && len(ids) > len(w.ids) {
				w.ids = ids
			}
		}

		statuses, allTerminal, anyMissing := w.loadStatuses()

		// session-ID mode has no rival PID to watch, so a never-appearing file
		// would otherwise hang until --timeout. A session JSON is created before
		// the run is even visible to a caller, so a missing file on the first
		// poll means a bad/nonexistent ID — fail fast.
		if !havePID && firstPoll && anyMissing {
			w.printf("no such session (file not found) — check the ID\n")
			return waitExitUsage
		}
		firstPoll = false

		// Primary signal in --log mode: rival process death. By the time the
		// rival process exits, stdout has been flushed, so the output the skill
		// will read is complete.
		ralive := havePID && w.ralive(w.pid, w.pidStart)

		switch {
		case havePID && !ralive:
			// rival is gone. Re-read once before deciding: the process can
			// finalize the session JSON and then exit between our status read
			// and this liveness check, which would otherwise look like a crash.
			statuses, allTerminal, _ = w.loadStatuses()
			if allTerminal && len(w.ids) > 0 {
				return w.summarize(statuses)
			}
			w.printf("crashed: rival (pid %d) exited before finalizing sessions\n", w.pid)
			return waitExitCrashed
		case !havePID && allTerminal && len(w.ids) > 0:
			// session-ID mode: terminal status is the only signal we have.
			return w.summarize(statuses)
		}

		if !w.now().Before(deadline) {
			w.printf("still running after %s (rival pid %d)%s\n", w.timeout, w.pid, w.lastQueueLine())
			return waitExitTimeout
		}
		if w.heartbeat > 0 && w.now().Sub(start) >= w.heartbeat {
			w.printf("heartbeat: running for %s (rival pid %d)%s\n", w.heartbeat, w.pid, w.lastQueueLine())
			return waitExitRunning
		}

		select {
		case <-ctx.Done():
			w.printf("interrupted while waiting\n")
			return waitExitCrashed
		case <-time.After(w.poll):
		}
	}
}

// loadStatuses reads the current status of every watched session. Returns the
// statuses, whether all are terminal, and whether any session file is missing.
func (w *waiter) loadStatuses() (statuses []sessionStatus, allTerminal, anyMissing bool) {
	statuses = make([]sessionStatus, len(w.ids))
	allTerminal = true
	for i, id := range w.ids {
		st := w.loadSession(id)
		st.ID = id
		statuses[i] = st
		if !st.found {
			anyMissing = true
		}
		if !st.terminal() {
			allTerminal = false
		}
	}
	return statuses, allTerminal, anyMissing
}

// summarize prints one line per session and returns 0 if all completed, else 2.
func (w *waiter) summarize(statuses []sessionStatus) int {
	code := waitExitCompleted
	for _, s := range statuses {
		exit := "-"
		if s.ExitCode != nil {
			exit = fmt.Sprintf("%d", *s.ExitCode)
		}
		id := s.ID
		if len(id) > 8 {
			id = id[:8]
		}
		line := fmt.Sprintf("%s %s exit=%s %s", id, s.Status, exit, s.Duration)
		if s.ErrorMsg != "" {
			line += " — " + s.ErrorMsg
		}
		w.printf("%s\n", line)
		if s.Status != "completed" {
			code = waitExitFailed
		}
	}
	return code
}

func (w *waiter) printf(format string, a ...any) {
	_, _ = fmt.Fprintf(w.out, format, a...)
}

// lastQueueLine returns the last "rival queue:" progress line from the log file
// (for the timeout message), prefixed with " — ", or "" if none.
func (w *waiter) lastQueueLine() string {
	if w.logFile == "" {
		return ""
	}
	data, err := os.ReadFile(w.logFile)
	if err != nil {
		return ""
	}
	last := ""
	for line := range strings.SplitSeq(string(data), "\n") {
		if len(line) >= 12 && line[:12] == "rival queue:" {
			last = line
		}
	}
	if last == "" {
		return ""
	}
	return " — " + last
}

// parseLogFile extracts the detached rival PID and the de-duplicated set of
// session IDs from a run's stderr file. The PID's start time is pinned now so a
// later PID-reuse cannot make a recycled PID look like our still-running rival.
func parseLogFile(path string) (pid int, pidStart int64, ids []string, err error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, nil, fmt.Errorf("read log file %q: %w", path, err)
	}
	text := string(data)

	if m := detachedPIDRe.FindStringSubmatch(text); m != nil {
		_, _ = fmt.Sscanf(m[1], "%d", &pid)
	}
	if pid > 0 {
		pidStart, _ = procinfo.StartNanos(pid)
	}

	// Only collect session IDs from THIS run's "starting …" markers. Startup
	// maintenance (ReapOrphans, queue ReapDead) logs old session IDs into the
	// same detached stderr; matching every "session" field would make wait
	// track stale sessions and mis-report a healthy run as failed/crashed.
	// The run's own start lines carry message:"starting …"; reaper lines carry
	// message:"reaping …". Require both fields on the same line.
	seen := map[string]bool{}
	for line := range strings.SplitSeq(text, "\n") {
		if !startingMarkerRe.MatchString(line) {
			continue
		}
		m := sessionIDRe.FindStringSubmatch(line)
		if m == nil || seen[m[1]] {
			continue
		}
		seen[m[1]] = true
		ids = append(ids, m[1])
	}

	if pid == 0 && len(ids) == 0 {
		return 0, 0, nil, fmt.Errorf("no detached pid or run session found in %q (run may have failed before launch)", path)
	}
	return pid, pidStart, ids, nil
}

// loadSessionStatus reads a session JSON directly (same approach as the queue's
// sessionLive — avoids a session-package import cycle for the minimal fields).
func loadSessionStatus(id string) sessionStatus {
	data, err := os.ReadFile(filepath.Join(config.SessionDirPath(), id+".json"))
	if err != nil {
		return sessionStatus{}
	}
	var s struct {
		Status   string `json:"status"`
		ExitCode *int   `json:"exit_code"`
		Duration string `json:"duration"`
		ErrorMsg string `json:"error"`
	}
	if json.Unmarshal(data, &s) != nil {
		return sessionStatus{}
	}
	return sessionStatus{
		Status:   s.Status,
		ExitCode: s.ExitCode,
		Duration: s.Duration,
		ErrorMsg: s.ErrorMsg,
		found:    true,
	}
}
