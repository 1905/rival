package review

import (
	"context"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
)

// A partial MarkRunning failure must not strand earlier sessions in "running"
// with no process behind them. The cmd surface used a copy of this waiter that
// lacked the rollback; both now share this one.
func TestWaitForGroupSlotRollsBackPartialStart(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	first, err := session.NewQueued("codex", "review", config.GPT56SolModel, "high", t.TempDir(), "p", "", "grp")
	if err != nil {
		t.Fatal(err)
	}
	// An ID containing a path separator makes Save target a directory that
	// does not exist, so MarkRunning fails for this member after the first one
	// already flipped to running.
	broken := &session.Session{ID: "missing-dir/broken", GroupID: "grp", Status: "queued"}

	sessions := []*session.Session{first, broken}
	release, err := WaitForGroupSlot(context.Background(), true, sessions, sessions, t.TempDir(), "grp", "review")
	if err == nil {
		if release != nil {
			release()
		}
		t.Fatal("expected an error when a session cannot be marked running")
	}

	reloaded, loadErr := session.Load(first.ID)
	if loadErr != nil {
		t.Fatalf("reload first session: %v", loadErr)
	}
	if reloaded.Status == "running" {
		t.Errorf("first session left %q with no process; it must be rolled back", reloaded.Status)
	}
}

func TestFormatSkipped_DistinguishesModels(t *testing.T) {
	if got := formatSkipped(nil); got != "none" {
		t.Fatalf("formatSkipped(nil) = %q, want none", got)
	}
	got := formatSkipped([]SkippedCLI{
		{CLI: "opencode", Model: config.KimiModel, Reason: "failed"},
		{CLI: config.GrokLabel, Model: config.GrokModel, Reason: "unavailable"},
	})
	if !strings.Contains(got, "kimi-k3: failed") || !strings.Contains(got, "grok: unavailable") {
		t.Fatalf("skipped models are not distinguishable: %s", got)
	}
}
