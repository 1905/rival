package dashboard

import (
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

func TestGroupDetailReservesSpaceForEveryPlanLog(t *testing.T) {
	writeLog := func(name, content string) string {
		t.Helper()
		path := t.TempDir() + "/" + name
		if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
		return path
	}
	item := &displayItem{Sessions: []*session.Session{
		{ID: "sol", GroupID: "paired-plan", CLI: "codex", Model: config.GPT56SolModel, Mode: "plan", Effort: "ultra", Status: "completed", Prompt: strings.Repeat("long plan context ", 100), LogFile: writeLog("sol.log", "sol output\n")},
		{ID: "claude", GroupID: "paired-plan", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", Effort: "low", Status: "completed", LogFile: writeLog("claude.log", "claude output\n")},
	}}

	// The viewport scrolls the log, so there is no line budget: every member's
	// heading AND body must be present in full.
	got := buildGroupLogContent(item, 80)
	for _, want := range []string{
		"SOL REVIEW · EFFORT ultra", "sol output",
		"CLAUDE REVIEW · EFFORT low", "claude output",
	} {
		if !strings.Contains(got, want) {
			t.Fatalf("grouped log content omitted %q:\n%s", want, got)
		}
	}
}

func TestSingleDetailStillShowsItsEffort(t *testing.T) {
	item := &displayItem{Sessions: []*session.Session{{
		ID:        "single",
		CLI:       "codex",
		Model:     config.GPT56SolModel,
		Mode:      "review",
		Effort:    "ultra",
		Status:    "completed",
		StartTime: time.Now(),
	}}}
	got := renderDetailMeta(item, 80, 30, false, nil)
	if !strings.Contains(got, "Effort") || !strings.Contains(got, "ultra") {
		t.Fatalf("single-session detail meta omitted effort:\n%s", got)
	}
	// "Reviewer" and "Model" used to render the same value twice.
	if n := strings.Count(got, "Model"); n != 1 {
		t.Fatalf("expected exactly one Model row, got %d:\n%s", n, got)
	}
	if strings.Contains(got, "Reviewer") {
		t.Fatalf("duplicate Reviewer row still present:\n%s", got)
	}
}

func TestGroupLogsDistinguishJudgeAndIncludeAllMembers(t *testing.T) {
	writeLog := func(name, content string) string {
		t.Helper()
		path := t.TempDir() + "/" + name
		if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
		return path
	}
	sessions := []*session.Session{
		{CLI: "codex", Model: config.GPT56SolModel, Mode: "megareview", Status: "completed", LogFile: writeLog("review.log", "review body\n")},
		{CLI: "codex", Model: config.GPT56SolModel, Mode: "consilium", Status: "completed", LogFile: writeLog("judge.log", "judge body\n")},
	}
	path, err := createPublicGroupLogView(sessions)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = os.Remove(path) }()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	got := string(data)
	for _, want := range []string{"SOL REVIEW", "review body", "SOL JUDGE", "judge body"} {
		if !strings.Contains(got, want) {
			t.Fatalf("combined group log missing %q:\n%s", want, got)
		}
	}
}

func TestDetailViewHandlesTinyTerminal(t *testing.T) {
	m := New()
	defer m.cancel()
	m.viewMode = viewDetail
	m.items = []displayItem{{Sessions: []*session.Session{
		{ID: "tiny", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", Status: "running"},
	}}}
	m.allItems = m.items

	updated, _ := m.Update(tea.WindowSizeMsg{Width: 80, Height: 1})
	m = updated.(Model)

	_ = m.View() // must not panic
	if h := m.logView.Height(); h < 1 {
		t.Fatalf("viewport height %d, want >= 1", h)
	}
}
