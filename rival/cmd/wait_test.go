package cmd

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestParseLogFile(t *testing.T) {
	id1 := "11111111-1111-1111-1111-111111111111"
	id2 := "22222222-2222-2222-2222-222222222222"

	tests := []struct {
		name    string
		content string
		wantPID int
		wantIDs []string
		wantErr bool
	}{
		{
			name:    "single CLI",
			content: "rival: detached pid=4242\n" + `{"level":"info","session":"` + id1 + `","effort":"low","mode":"review","message":"starting codex (command mode)"}` + "\n",
			wantPID: 4242,
			wantIDs: []string{id1},
		},
		{
			name: "multi-id deduped",
			content: "rival: detached pid=99\n" +
				`{"session":"` + id1 + `","cli":"codex","message":"starting codex"}` + "\n" +
				`{"session":"` + id2 + `","cli":"claude","message":"starting claude"}` + "\n" +
				`{"session":"` + id1 + `","cli":"codex","message":"starting codex"}` + "\n", // dup
			wantPID: 99,
			wantIDs: []string{id1, id2},
		},
		{
			// THE regression: a ReapOrphans line carries an old session ID but
			// message:"reaping …" — it must be ignored, only the run's
			// "starting" session counted.
			name: "ignores reaper session IDs",
			content: "rival: detached pid=7\n" +
				`{"session":"` + id2 + `","pid":111,"status":"running","message":"reaping orphaned session"}` + "\n" +
				`{"session":"` + id1 + `","effort":"low","message":"starting codex (command mode)"}` + "\n",
			wantPID: 7,
			wantIDs: []string{id1}, // NOT id2
		},
		{
			name:    "no pid no run session is error",
			content: "some unrelated output\n" + `{"session":"` + id2 + `","message":"reaping orphaned session"}` + "\n",
			wantErr: true, // only a reaper line → no current-run session, no pid
		},
		{
			name:    "ids without pid (non-detached) still parse",
			content: `{"session":"` + id1 + `","message":"starting codex (command mode)"}` + "\n",
			wantPID: 0,
			wantIDs: []string{id1},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			p := filepath.Join(t.TempDir(), "err.log")
			if err := os.WriteFile(p, []byte(tt.content), 0600); err != nil {
				t.Fatal(err)
			}
			pid, _, ids, err := parseLogFile(p)
			if tt.wantErr {
				if err == nil {
					t.Fatal("expected error, got nil")
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if pid != tt.wantPID {
				t.Errorf("pid=%d, want %d", pid, tt.wantPID)
			}
			if strings.Join(ids, ",") != strings.Join(tt.wantIDs, ",") {
				t.Errorf("ids=%v, want %v", ids, tt.wantIDs)
			}
		})
	}

	t.Run("missing file errors", func(t *testing.T) {
		if _, _, _, err := parseLogFile(filepath.Join(t.TempDir(), "nope")); err == nil {
			t.Error("expected error for missing file")
		}
	})
}

// fakeStore returns canned sessionStatus values by id.
func fakeStore(m map[string]sessionStatus) func(string) sessionStatus {
	return func(id string) sessionStatus { return m[id] }
}

func ptr(i int) *int { return &i }

func TestWaiterRun(t *testing.T) {
	const aliveStart int64 = 123
	completed := sessionStatus{Status: "completed", ExitCode: ptr(0), Duration: "5s", found: true}
	failed := sessionStatus{Status: "failed", ExitCode: ptr(1), Duration: "2s", ErrorMsg: "run timeout after 5s (RIVAL_RUN_TIMEOUT)", found: true}
	running := sessionStatus{Status: "running", found: true}

	tests := []struct {
		name     string
		pid      int
		ids      []string
		store    map[string]sessionStatus
		alive    bool // rival pid alive?
		wantCode int
		wantOut  string // substring expected in summary
	}{
		{
			name: "log-mode all completed after rival dies",
			pid:  7, ids: []string{"a"},
			store:    map[string]sessionStatus{"a": completed},
			alive:    false,
			wantCode: waitExitCompleted,
			wantOut:  "completed exit=0",
		},
		{
			name: "log-mode failed session → exit 2",
			pid:  7, ids: []string{"a"},
			store:    map[string]sessionStatus{"a": failed},
			alive:    false,
			wantCode: waitExitFailed,
			wantOut:  "RIVAL_RUN_TIMEOUT",
		},
		{
			name: "log-mode rival dead but session stuck running → crash",
			pid:  7, ids: []string{"a"},
			store:    map[string]sessionStatus{"a": running},
			alive:    false,
			wantCode: waitExitCrashed,
			wantOut:  "crashed",
		},
		{
			name: "session-id mode all terminal",
			pid:  0, ids: []string{"a", "b"},
			store:    map[string]sessionStatus{"a": completed, "b": failed},
			alive:    false,
			wantCode: waitExitFailed,
			wantOut:  "failed exit=1",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var buf bytes.Buffer
			w := &waiter{
				pid:         tt.pid,
				pidStart:    aliveStart,
				ids:         tt.ids,
				poll:        time.Millisecond,
				timeout:     2 * time.Second,
				loadSession: fakeStore(tt.store),
				ralive:      func(int, int64) bool { return tt.alive },
				now:         time.Now,
				out:         &buf,
			}
			code := w.run(context.Background())
			if code != tt.wantCode {
				t.Errorf("exit code=%d, want %d (out: %q)", code, tt.wantCode, buf.String())
			}
			if tt.wantOut != "" && !strings.Contains(buf.String(), tt.wantOut) {
				t.Errorf("output %q missing %q", buf.String(), tt.wantOut)
			}
		})
	}
}

func TestIsSessionID(t *testing.T) {
	good := "11111111-1111-1111-1111-111111111111"
	bad := []string{
		"../../etc/passwd",
		"not-a-uuid",
		"11111111-1111-1111-1111-111111111111/x",
		"",
		"11111111111111111111111111111111", // no dashes
	}
	if !isSessionID(good) {
		t.Errorf("expected %q to be a valid session ID", good)
	}
	for _, b := range bad {
		if isSessionID(b) {
			t.Errorf("expected %q to be rejected", b)
		}
	}
}

func TestWaiterRun_SessionIDModeMissingFailsFast(t *testing.T) {
	// No PID, session file never found → must fail fast (usage), not hang.
	var buf bytes.Buffer
	w := &waiter{
		pid:         0,
		ids:         []string{"11111111-1111-1111-1111-111111111111"},
		poll:        time.Millisecond,
		timeout:     time.Hour,                                             // would hang for an hour without the fast-fail
		loadSession: func(string) sessionStatus { return sessionStatus{} }, // found=false
		ralive:      func(int, int64) bool { return false },
		now:         time.Now,
		out:         &buf,
	}
	if code := w.run(context.Background()); code != waitExitUsage {
		t.Errorf("exit=%d, want %d (out: %q)", code, waitExitUsage, buf.String())
	}
}

func TestWaiterRun_NoFalseCrashOnFinalizeRace(t *testing.T) {
	// rival is dead, and the session is non-terminal on the first read but
	// terminal on the re-read (it finalized + exited between our reads). The
	// re-read must turn this into a clean summary, NOT a false crash.
	var buf bytes.Buffer
	reads := 0
	w := &waiter{
		pid:     7,
		ids:     []string{"a"},
		poll:    time.Millisecond,
		timeout: 2 * time.Second,
		loadSession: func(string) sessionStatus {
			reads++
			if reads == 1 {
				return sessionStatus{Status: "running", found: true} // stale
			}
			return sessionStatus{Status: "completed", ExitCode: ptr(0), Duration: "5s", found: true}
		},
		ralive: func(int, int64) bool { return false }, // process already gone
		now:    time.Now,
		out:    &buf,
	}
	if code := w.run(context.Background()); code != waitExitCompleted {
		t.Errorf("exit=%d, want %d (false crash not avoided) — out: %q", code, waitExitCompleted, buf.String())
	}
}

func TestWaiterRun_Timeout(t *testing.T) {
	// rival alive forever, sessions never terminal → must hit timeout (exit 4).
	var buf bytes.Buffer
	base := time.Now()
	calls := 0
	w := &waiter{
		pid:         7,
		ids:         []string{"a"},
		poll:        time.Millisecond,
		timeout:     50 * time.Millisecond,
		loadSession: func(string) sessionStatus { return sessionStatus{Status: "running", found: true} },
		ralive:      func(int, int64) bool { return true },
		now: func() time.Time {
			// advance fast so the deadline trips deterministically
			calls++
			return base.Add(time.Duration(calls) * 20 * time.Millisecond)
		},
		out: &buf,
	}
	if code := w.run(context.Background()); code != waitExitTimeout {
		t.Errorf("exit code=%d, want %d (out: %q)", code, waitExitTimeout, buf.String())
	}
	if !strings.Contains(buf.String(), "still running") {
		t.Errorf("expected 'still running', got %q", buf.String())
	}
}
