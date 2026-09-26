package dashboard

import (
	"testing"
	"time"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
	"github.com/1F47E/rival/internal/sessionview"
)

// The TUI and the web dashboard must report the same derived values for the
// same sessions. Both now read through internal/sessionview, so this asserts
// the TUI's row helpers return exactly what the shared package produces.
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
	if got, want := groupKindLabel(item), sessionview.Kind(members); got != want {
		t.Errorf("kind: TUI %q, shared %q", got, want)
	}
	if got, want := groupModels(item), sessionview.JoinLabels(sessionview.EngineLabels(members), " + "); got != want {
		t.Errorf("models: TUI %q, shared %q", got, want)
	}
	if got, want := groupElapsed(item), sessionview.Elapsed(members); got != want {
		t.Errorf("elapsed: TUI %q, shared %q", got, want)
	}

	// The span covers both members. The old TUI reported the longest single
	// member instead, which is how the two dashboards drifted apart.
	if got := groupElapsed(item); got != "7m0s" {
		t.Errorf("group elapsed = %q, want the 7m0s span rather than a 4m member", got)
	}
	// An antislop group must not be labelled a plan review.
	if got := groupIcon(item); got != iconAntislop+" slop" {
		t.Errorf("group icon = %q, want the antislop icon", got)
	}
}
