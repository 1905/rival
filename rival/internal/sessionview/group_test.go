package sessionview

import (
	"reflect"
	"testing"
	"time"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

func sess(id, groupID, status, mode, cli, model, effort string) *session.Session {
	return &session.Session{
		ID:      id,
		GroupID: groupID,
		Status:  status,
		Mode:    mode,
		CLI:     cli,
		Model:   model,
		Effort:  effort,
	}
}

func TestGroupBucketsAndKeys(t *testing.T) {
	solo := sess("s1", "", "completed", "review", "codex", config.GPT56SolModel, "high")
	a := sess("a1", "grp", "completed", "plan", "codex", config.GPT56SolModel, "xhigh")
	b := sess("b1", "grp", "completed", "plan", "claude", config.ClaudeModel, "xhigh")

	buckets := Group([]*session.Session{solo, a, b})
	if len(buckets) != 2 {
		t.Fatalf("got %d buckets, want 2", len(buckets))
	}
	if buckets[0].Key != "solo:s1" {
		t.Errorf("solo key = %q, want solo:s1", buckets[0].Key)
	}
	if buckets[1].Key != "grp" {
		t.Errorf("group key = %q, want grp", buckets[1].Key)
	}
	if len(buckets[1].Sessions) != 2 {
		t.Errorf("group has %d members, want 2", len(buckets[1].Sessions))
	}
}

func TestGroupPreservesFirstAppearanceOrder(t *testing.T) {
	first := sess("x", "g2", "completed", "review", "codex", config.GPT56SolModel, "high")
	second := sess("y", "g1", "completed", "review", "codex", config.GPT56SolModel, "high")
	third := sess("z", "g2", "completed", "review", "claude", config.ClaudeModel, "high")

	buckets := Group([]*session.Session{first, second, third})
	if len(buckets) != 2 || buckets[0].Key != "g2" || buckets[1].Key != "g1" {
		t.Fatalf("bucket order = %v, want [g2 g1]", []string{buckets[0].Key, buckets[1].Key})
	}
}

func TestGroupDoesNotMutateInput(t *testing.T) {
	a := sess("a", "g", "completed", "plan", "codex", config.GPT56SolModel, "high")
	b := sess("b", "g", "running", "plan", "claude", config.ClaudeModel, "low")
	input := []*session.Session{a, b}
	before := []session.Session{*a, *b}

	Group(input)

	if !reflect.DeepEqual(*a, before[0]) || !reflect.DeepEqual(*b, before[1]) {
		t.Error("Group mutated its input sessions")
	}
}

func TestStatusTier(t *testing.T) {
	tests := []struct {
		name     string
		statuses []string
		want     string
	}{
		{"running wins", []string{"completed", "running", "queued"}, "running"},
		{"queued beats failed", []string{"failed", "queued", "completed"}, "queued"},
		{"failed beats completed", []string{"completed", "failed"}, "failed"},
		{"all completed", []string{"completed", "completed"}, "completed"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var sessions []*session.Session
			for i, st := range tt.statuses {
				sessions = append(sessions, sess(string(rune('a'+i)), "g", st, "review", "codex", config.GPT56SolModel, "high"))
			}
			if got := Status(sessions); got != tt.want {
				t.Errorf("Status = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestKindPrecedence(t *testing.T) {
	tests := []struct {
		name  string
		modes []string
		want  string
	}{
		{"antislop solo", []string{"antislop"}, "antislop"},
		{"antislop group", []string{"antislop", "antislop"}, "antislop"},
		{"antislop wins over plan", []string{"plan", "antislop"}, "antislop"},
		{"plan", []string{"plan", "plan"}, "plan"},
		{"review is megareview", []string{"review", "review"}, "megareview"},
		{"raw is megareview", []string{"raw"}, "megareview"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var sessions []*session.Session
			for i, m := range tt.modes {
				sessions = append(sessions, sess(string(rune('a'+i)), "g", "completed", m, "codex", config.GPT56SolModel, "high"))
			}
			if got := Kind(sessions); got != tt.want {
				t.Errorf("Kind = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestEngineLabelsDedupInOrder(t *testing.T) {
	sessions := []*session.Session{
		sess("a", "g", "completed", "plan", "codex", config.GPT56SolModel, "high"),
		sess("b", "g", "completed", "plan", "claude", config.ClaudeModel, "high"),
		sess("c", "g", "completed", "plan", "codex", config.GPT56SolModel, "high"),
	}
	got := EngineLabels(sessions)
	want := []string{config.SolLabel, config.ClaudeLabel}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("EngineLabels = %v, want %v", got, want)
	}
	if JoinLabels(got, "+") != config.SolLabel+"+"+config.ClaudeLabel {
		t.Errorf("JoinLabels with + = %q", JoinLabels(got, "+"))
	}
	if JoinLabels(got, " + ") != config.SolLabel+" + "+config.ClaudeLabel {
		t.Errorf("JoinLabels with space = %q", JoinLabels(got, " + "))
	}
}

func TestEffort(t *testing.T) {
	same := []*session.Session{
		sess("a", "g", "completed", "plan", "codex", config.GPT56SolModel, "xhigh"),
		sess("b", "g", "completed", "plan", "claude", config.ClaudeModel, "xhigh"),
	}
	if got := Effort(same); got != "xhigh" {
		t.Errorf("Effort = %q, want xhigh", got)
	}

	mixed := []*session.Session{
		sess("a", "g", "completed", "plan", "codex", config.GPT56SolModel, "xhigh"),
		sess("b", "g", "completed", "plan", "claude", config.ClaudeModel, "low"),
	}
	if got := Effort(mixed); got != "mixed" {
		t.Errorf("Effort = %q, want mixed", got)
	}

	if got := Effort(nil); got != "" {
		t.Errorf("Effort(nil) = %q, want empty", got)
	}
}

// Elapsed is the wall-clock span of the whole group. The TUI used to report
// the longest single member instead, which is why the two dashboards disagreed.
func TestElapsedSpansTheWholeGroup(t *testing.T) {
	base := time.Now().Add(-30 * time.Minute)
	firstEnd := base.Add(4 * time.Minute)
	secondStart := base.Add(4 * time.Minute)
	secondEnd := base.Add(7 * time.Minute)

	sequential := []*session.Session{
		{ID: "a", Status: "completed", StartTime: base, EndTime: &firstEnd},
		{ID: "b", Status: "completed", StartTime: secondStart, EndTime: &secondEnd},
	}
	if got := Elapsed(sequential); got != "7m0s" {
		t.Errorf("sequential Elapsed = %q, want 7m0s (span, not the 4m longest member)", got)
	}

	overlapEnd := base.Add(10 * time.Minute)
	innerStart := base.Add(2 * time.Minute)
	innerEnd := base.Add(5 * time.Minute)
	overlapping := []*session.Session{
		{ID: "a", Status: "completed", StartTime: base, EndTime: &overlapEnd},
		{ID: "b", Status: "completed", StartTime: innerStart, EndTime: &innerEnd},
	}
	if got := Elapsed(overlapping); got != "10m0s" {
		t.Errorf("overlapping Elapsed = %q, want 10m0s", got)
	}
}

func TestElapsedUsesDurationFallbackAndQueuedAt(t *testing.T) {
	base := time.Now().Add(-20 * time.Minute)
	withDuration := []*session.Session{
		{ID: "a", Status: "completed", StartTime: base, Duration: "3m0s"},
	}
	if got := Elapsed(withDuration); got != "3m0s" {
		t.Errorf("Duration fallback Elapsed = %q, want 3m0s", got)
	}

	queuedAt := time.Now().Add(-10 * time.Minute)
	queued := []*session.Session{
		{ID: "a", Status: "queued", QueuedAt: &queuedAt},
	}
	got := Elapsed(queued)
	parsed, err := time.ParseDuration(got)
	if err != nil {
		t.Fatalf("queued Elapsed = %q, not a duration: %v", got, err)
	}
	if parsed < 9*time.Minute || parsed > 12*time.Minute {
		t.Errorf("queued Elapsed = %v, want about 10m counted from QueuedAt", parsed)
	}
}

func TestElapsedWithoutStartIsDash(t *testing.T) {
	if got := Elapsed([]*session.Session{{ID: "a", Status: "queued"}}); got != "-" {
		t.Errorf("Elapsed = %q, want -", got)
	}
}
