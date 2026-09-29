package session

import (
	"testing"

	"github.com/1905/rival/internal/config"
)

// deadPID is above the darwin PID ceiling (~99998) and far beyond typical linux
// pid_max defaults, so procinfo.Alive always reports it dead; the bogus start
// time guards the unlikely platform where such a PID could exist.
const (
	deadPID      = 999999
	deadPIDStart = int64(12345)
)

func reloadByID(t *testing.T, id string) *Session {
	t.Helper()
	for _, s := range LoadAll() {
		if s.ID == id {
			return s
		}
	}
	t.Fatalf("session %s not found after reap", id)
	return nil
}

// A running session whose provider child already exited but whose owning rival
// is still alive is mid-finalization, not orphaned — the reaper must leave it
// for the owner to complete. This is the end-of-run race that stomped a
// successful run to failed when a concurrent rival invocation reaped inside
// the provider-exit → status-write window.
func TestReapOrphansSparesDeadProviderWithLiveOwner(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	s, err := NewQueued("opencode", "raw", config.KimiModel, "max", t.TempDir(), "prompt", "", "")
	if err != nil {
		t.Fatal(err)
	}
	// New() used to create a running session outright; MarkRunning reproduces
	// that state now that queued is the only entry point.
	if err := s.MarkRunning(); err != nil {
		t.Fatal(err)
	}
	// create() records this test process as the (alive) owner; simulate the
	// provider child having already exited.
	s.PID = deadPID
	s.PIDStart = deadPIDStart
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}

	ReapOrphans()

	if got := reloadByID(t, s.ID).Status; got != "running" {
		t.Errorf("status = %q, want running (live owner must block the reap)", got)
	}
}

func TestReapOrphansReapsWhenOwnerAndProviderDead(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	s, err := NewQueued("opencode", "raw", config.KimiModel, "max", t.TempDir(), "prompt", "", "")
	if err != nil {
		t.Fatal(err)
	}
	// New() used to create a running session outright; MarkRunning reproduces
	// that state now that queued is the only entry point.
	if err := s.MarkRunning(); err != nil {
		t.Fatal(err)
	}
	s.PID = deadPID
	s.PIDStart = deadPIDStart
	s.OwnerPID = deadPID
	s.OwnerPIDStart = deadPIDStart
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}

	ReapOrphans()

	got := reloadByID(t, s.ID)
	if got.Status != "failed" {
		t.Errorf("status = %q, want failed", got.Status)
	}
	if got.ErrorMsg != "orphaned (process dead)" {
		t.Errorf("error = %q, want orphaned (process dead)", got.ErrorMsg)
	}
	if got.Prompt != "prompt" {
		t.Errorf("prompt = %q, want original prompt preserved", got.Prompt)
	}
}

// Sessions written by releases without owner tracking (OwnerPID 0) keep the
// provider-only liveness check.
func TestReapOrphansReapsLegacySessionWithoutOwner(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	s, err := NewQueued("opencode", "raw", config.KimiModel, "max", t.TempDir(), "prompt", "", "")
	if err != nil {
		t.Fatal(err)
	}
	// New() used to create a running session outright; MarkRunning reproduces
	// that state now that queued is the only entry point.
	if err := s.MarkRunning(); err != nil {
		t.Fatal(err)
	}
	s.PID = deadPID
	s.PIDStart = deadPIDStart
	s.OwnerPID = 0
	s.OwnerPIDStart = 0
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}

	ReapOrphans()

	if got := reloadByID(t, s.ID).Status; got != "failed" {
		t.Errorf("status = %q, want failed (legacy sessions keep old behavior)", got)
	}
}
