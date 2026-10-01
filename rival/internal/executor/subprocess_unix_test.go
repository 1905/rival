//go:build !windows

package executor

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/1905/rival/internal/session"
)

// TestRunSubprocess_TimeoutKillsLauncherGrandchild reproduces the npm `codex`
// launcher shape: a wrapper that spawns the real binary with inherited stdio
// and cannot forward SIGKILL. The grandchild ignores SIGTERM and keeps the
// stdout/stderr pipes open. A context timeout must still return promptly and
// leave the grandchild dead — otherwise the queue slot is never released.
func TestRunSubprocess_TimeoutKillsLauncherGrandchild(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	dir := t.TempDir()
	pidFile := filepath.Join(dir, "grandchild.pid")
	launcher := filepath.Join(dir, "launcher.sh")
	// The subshell ignores SIGTERM; SIG_IGN survives exec, so the sleep does too.
	script := "#!/bin/sh\n" +
		"( trap '' TERM; exec sleep 20 ) &\n" +
		"echo $! > " + pidFile + "\n" +
		"echo launcher-started\n" +
		"sleep 20\n"
	if err := os.WriteFile(launcher, []byte(script), 0o700); err != nil {
		t.Fatalf("write launcher: %v", err)
	}

	sess, err := session.NewQueued("test", "raw", "none", "low", dir, "", "", "")
	if err != nil {
		t.Fatalf("create session: %v", err)
	}
	if err := sess.MarkRunning(); err != nil {
		t.Fatalf("mark running: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	start := time.Now()
	_, _ = RunSubprocess(ctx, sess, launcher, nil, nil, "", nil)
	elapsed := time.Since(start)

	grandchild := readPID(t, pidFile)
	t.Cleanup(func() { _ = syscall.Kill(grandchild, syscall.SIGKILL) })

	if elapsed > 4*time.Second {
		t.Fatalf("RunSubprocess hung %v after the context timeout (grandchild held the pipes)", elapsed)
	}

	// The grandchild is reparented to init once the launcher dies, so it is
	// reaped asynchronously; poll briefly for it to disappear.
	deadline := time.Now().Add(3 * time.Second)
	for {
		if err := syscall.Kill(grandchild, 0); errors.Is(err, syscall.ESRCH) {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("grandchild %d still alive after the run was cancelled", grandchild)
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func readPID(t *testing.T, path string) int {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read grandchild pid: %v", err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		t.Fatalf("parse grandchild pid %q: %v", data, err)
	}
	return pid
}
