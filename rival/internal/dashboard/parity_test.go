package dashboard

import (
	"testing"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
	"github.com/1905/rival/internal/sessionview"
)

// The TUI row helpers must report exactly the values internal/sessionview
// derives for the same sessions, and those values are pinned directly below.
func TestTUIRowValuesMatchSharedDerivations(t *testing.T) {
	base := time.Now().Add(-20 * time.Minute)
	firstEnd := base.Add(4 * time.Minute)
	secondStart := firstEnd
	secondEnd := secondStart.Add(3 * time.Minute)

	members := []*session.Session{
		{ID: "a", GroupID: "g", Mode: session.ModeAntislop, Status: "completed", CLI: "codex", Model: config.GPT56SolModel, Effort: "xhigh", StartTime: base, EndTime: &firstEnd},
		{ID: "b", GroupID: "g", Mode: session.ModeAntislop, Status: "completed", CLI: "claude", Model: config.ClaudeModel, Effort: "xhigh", StartTime: secondStart, EndTime: &secondEnd},
	}
	item := &displayItem{Sessions: members}

	if got, want := groupStatus(item), sessionview.Status(members); got != want {
		t.Errorf("status: TUI %q, shared %q", got, want)
	}
	if got, want := groupEffort(item), sessionview.Effort(members); got != want {
		t.Errorf("effort: TUI %q, shared %q", got, want)
	}
	if got, want := kindLabel(item), shortKind(sessionview.Kind(members)); got != want {
		t.Errorf("kind: TUI %q, shared %q", got, want)
	}
	// The TUI shows raw model ids, not the shared public labels: the first
	// requested model plus a count of the other distinct ids.
	if got, want := groupModelName(item), config.GPT56SolModel+" +1"; got != want {
		t.Errorf("models: TUI %q, want %q", got, want)
	}
	if got, want := groupElapsed(item), sessionview.Elapsed(members); got != want {
		t.Errorf("elapsed: TUI %q, shared %q", got, want)
	}

	// The span covers both members. The old TUI reported the longest single
	// member instead.
	if got := groupElapsed(item); got != "7m0s" {
		t.Errorf("group elapsed = %q, want the 7m0s span rather than a 4m member", got)
	}
	if got := groupStatus(item); got != "completed" {
		t.Errorf("group status = %q, want completed", got)
	}
	if got := groupEffort(item); got != "xhigh" {
		t.Errorf("group effort = %q, want xhigh", got)
	}
	// An antislop group must not be labelled a plan review.
	if got := kindLabel(item); got != "slop" {
		t.Errorf("group kind = %q, want slop", got)
	}
}
