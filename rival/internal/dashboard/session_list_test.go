package dashboard

import (
	"os"
	"strings"
	"testing"
	"time"

	"charm.land/lipgloss/v2"
	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

func TestCLILabelUsesPublicModelNames(t *testing.T) {
	tests := []struct {
		cli, model, want string
	}{
		{"codex", config.GPT56SolModel, iconSol + " " + config.SolLabel},
		{"opencode", config.KimiModel, iconOpencode + " kimi-k3"},
		{"claude", config.ClaudeModel, iconClaude + " " + config.ClaudeLabel},
		{"grok", config.GrokModel, iconGrok + " " + config.GrokLabel},
	}
	for _, tc := range tests {
		if got := cliLabel(tc.cli, tc.model, "review"); got != tc.want {
			t.Errorf("cliLabel(%q, %q) = %q, want %q", tc.cli, tc.model, got, tc.want)
		}
	}
	if got := cliLabel("claude", config.ClaudeModel, "plan"); got != iconPlan+" plan" {
		t.Errorf("live Claude plan label = %q, want %q", got, iconPlan+" plan")
	}
}

// Column alignment assumes every CLI icon occupies exactly one terminal cell.
// A double-width or ambiguous-width glyph would shift the REVIEWER column.
func TestCLIIconsAreSingleCellWide(t *testing.T) {
	icons := map[string]string{
		"sol":      iconSol,
		"claude":   iconClaude,
		"opencode": iconOpencode,
		"plan":     iconPlan,
		"grok":     iconGrok,
	}
	for name, icon := range icons {
		if got := lipgloss.Width(icon); got != 1 {
			t.Errorf("icon %s (%q) width = %d, want 1", name, icon, got)
		}
	}
}

func TestCreatePublicLogViewSanitizesRuntimeMetadata(t *testing.T) {
	rawPath := t.TempDir() + "/raw.log"
	raw := "OpenAI Codex v1\n--------\nmodel: " + config.GPT56SolModel + "\n--------\n"
	if err := os.WriteFile(rawPath, []byte(raw), 0o600); err != nil {
		t.Fatal(err)
	}
	viewPath, err := createPublicLogView(&session.Session{CLI: "codex", Model: config.GPT56SolModel, LogFile: rawPath})
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = os.Remove(viewPath) }()
	data, err := os.ReadFile(viewPath)
	if err != nil {
		t.Fatal(err)
	}
	got := string(data)
	if strings.Contains(got, config.GPT56SolModel) || strings.Contains(got, "Codex") || !strings.Contains(got, "Sol runtime") {
		t.Fatalf("opened log view was not normalized: %q", got)
	}
}

func TestGroupIconReflectsSelectedReviewerCount(t *testing.T) {
	tests := []struct {
		name     string
		sessions []*session.Session
		want     string
	}{
		{
			name: "one model plus judge",
			sessions: []*session.Session{
				{Mode: "megareview"}, {Mode: "consilium"},
			},
			want: "❯ mega",
		},
		{
			name: "three models plus judge",
			sessions: []*session.Session{
				{Mode: "megareview"}, {Mode: "megareview"}, {Mode: "megareview"}, {Mode: "consilium"},
			},
			want: "❯❯❯ mega",
		},
		{
			name: "four models plus judge",
			sessions: []*session.Session{
				{Mode: "megareview"}, {Mode: "megareview"}, {Mode: "megareview"}, {Mode: "megareview"}, {Mode: "consilium"},
			},
			want: "❯❯❯❯ mega",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := groupIcon(&displayItem{Sessions: tc.sessions}); got != tc.want {
				t.Fatalf("groupIcon() = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestGroupEffortShowsMixedDefaults(t *testing.T) {
	item := &displayItem{Sessions: []*session.Session{
		{Effort: "ultra"},
		{Effort: "low"},
	}}
	if got := groupEffort(item); got != "mixed" {
		t.Fatalf("groupEffort() = %q, want mixed", got)
	}
	item.Sessions[1].Effort = "ultra"
	if got := groupEffort(item); got != "ultra" {
		t.Fatalf("groupEffort() = %q, want ultra", got)
	}
}

func TestPairedPlanGroupUsesPublicModelsAndPlanIcon(t *testing.T) {
	created := time.Now()
	later := created.Add(time.Millisecond)
	items := groupSessions([]*session.Session{
		// LoadAll returns newest first; grouping must restore requested order.
		{ID: "b", GroupID: "paired", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", QueuedAt: &later},
		{ID: "a", GroupID: "paired", CLI: "codex", Model: config.GPT56SolModel, Mode: "plan", QueuedAt: &created},
	})
	if len(items) != 1 || !items[0].IsGroup() {
		t.Fatalf("paired plan TUI items = %+v, want one group", items)
	}
	item := &items[0]
	if got := groupIcon(item); got != iconPlan+" plan" {
		t.Fatalf("paired plan icon = %q, want %q", got, iconPlan+" plan")
	}
	if got := groupCLIs(item); got != config.SolLabel+"+"+config.ClaudeLabel {
		t.Fatalf("paired plan reviewers = %q", got)
	}
	if got := groupModels(item); got != config.SolLabel+" + "+config.ClaudeLabel {
		t.Fatalf("paired plan models = %q", got)
	}
}

func TestSingletonClaudePlanRemainsLogicalPlanGroup(t *testing.T) {
	items := groupSessions([]*session.Session{
		{ID: "claude", GroupID: "degraded-plan", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", Status: "running"},
	})
	if len(items) != 1 || !items[0].IsGroup() {
		t.Fatalf("singleton plan item = %+v, want logical group", items)
	}
	if got := groupIcon(&items[0]); got != iconPlan+" plan" {
		t.Fatalf("singleton plan icon = %q", got)
	}
}
