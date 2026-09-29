package procinfo

import (
	"os"
	"testing"
)

func TestStartNanosSelf(t *testing.T) {
	n, ok := StartNanos(os.Getpid())
	if !ok || n <= 0 {
		t.Fatalf("StartNanos(self) = (%d, %v), want positive + ok (is this platform supported?)", n, ok)
	}
}

func TestAlive(t *testing.T) {
	pid := os.Getpid()
	start, ok := StartNanos(pid)
	if !ok {
		t.Skip("start time unsupported on this platform")
	}

	if !Alive(pid, start) {
		t.Error("Alive(self, correct start) = false, want true")
	}
	// A non-matching start time means the PID was recycled → treat as dead.
	if Alive(pid, start+1) {
		t.Error("Alive(self, wrong start) = true, want false (PID-reuse guard failed)")
	}
	// wantStart 0 = no recorded start → bare existence check.
	if !Alive(pid, 0) {
		t.Error("Alive(self, 0) = false, want true (best-effort fallback)")
	}
	// A dead PID is never alive.
	if Alive(1<<24, 0) {
		t.Error("Alive(dead pid, 0) = true, want false")
	}
	if Alive(0, 0) || Alive(-1, 0) {
		t.Error("Alive(non-positive pid) = true, want false")
	}
}

// Codex finding 1: stop authorization never degrades to an existence check.
func TestSameProcess(t *testing.T) {
	pid := os.Getpid()
	start, ok := StartNanos(pid)
	if !ok {
		t.Skip("start time unsupported on this platform")
	}
	if !SameProcess(pid, start) {
		t.Error("SameProcess(self, correct start) = false, want true")
	}
	if SameProcess(pid, start+1) {
		t.Error("SameProcess(self, wrong start) = true, want false")
	}
	if SameProcess(pid, 0) {
		t.Error("SameProcess(self, 0) = true, want false: no recorded start never authorizes")
	}
	if SameProcess(1<<24, start) || SameProcess(0, start) {
		t.Error("SameProcess(dead or non-positive pid) = true, want false")
	}
}
