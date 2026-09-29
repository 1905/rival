package session

import (
	"testing"
	"time"

	"github.com/1905/rival/internal/config"
)

func TestSortGroupMembersUsesCreationOrderAndPutsJudgeLast(t *testing.T) {
	created := time.Now()
	second := created.Add(time.Millisecond)
	third := second.Add(time.Millisecond)
	sessions := []*Session{
		{ID: "judge", CLI: "codex", Model: config.GPT56SolModel, Mode: "consilium", QueuedAt: &third},
		{ID: "claude", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", QueuedAt: &second},
		{ID: "sol", CLI: "codex", Model: config.GPT56SolModel, Mode: "plan", QueuedAt: &created},
	}

	SortGroupMembers(sessions)
	for i, want := range []string{"sol", "claude", "judge"} {
		if sessions[i].ID != want {
			t.Fatalf("member %d = %q, want %q", i, sessions[i].ID, want)
		}
	}
}

func TestSortGroupMembersUsesCuratedFallbackForLegacySessions(t *testing.T) {
	sessions := []*Session{
		{ID: "claude", CLI: "claude", Model: config.ClaudeModel, Mode: "plan"},
		{ID: "k3", CLI: "opencode", Model: config.KimiModel, Mode: "megareview"},
		{ID: "sol", CLI: "codex", Model: config.GPT56SolModel, Mode: "plan"},
	}

	SortGroupMembers(sessions)
	for i, want := range []string{"sol", "k3", "claude"} {
		if sessions[i].ID != want {
			t.Fatalf("legacy member %d = %q, want %q", i, sessions[i].ID, want)
		}
	}
}

func TestSortGroupMembersPreservesExplicitReviewerOrder(t *testing.T) {
	created := time.Now()
	later := created.Add(time.Millisecond)
	sessions := []*Session{
		{ID: "sol", CLI: "codex", Model: config.GPT56SolModel, Mode: "megareview", QueuedAt: &later},
		{ID: "k3", CLI: "opencode", Model: config.KimiModel, Mode: "megareview", QueuedAt: &created},
	}

	SortGroupMembers(sessions)
	if sessions[0].ID != "k3" || sessions[1].ID != "sol" {
		t.Fatalf("explicit reviewer order was not preserved: %s, %s", sessions[0].ID, sessions[1].ID)
	}
}

func TestGroupModelRankOrdersGrokAfterTheExistingNamedModels(t *testing.T) {
	tests := []struct {
		name    string
		session *Session
		want    int
	}{
		{"sol", &Session{CLI: "codex", Model: config.GPT56SolModel}, 0},
		{"kimi-k3", &Session{CLI: "opencode", Model: config.KimiModel}, 1},
		{"claude", &Session{CLI: "claude", Model: config.ClaudeModel}, 2},
		{"grok", &Session{CLI: config.GrokLabel, Model: config.GrokModel}, 3},
		{"unknown", &Session{CLI: "mystery", Model: "some-retired-model"}, 100},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := groupModelRank(tt.session); got != tt.want {
				t.Errorf("groupModelRank(%s) = %d, want %d", tt.name, got, tt.want)
			}
		})
	}
}

func TestSortGroupMembersPlacesGrokAfterClaude(t *testing.T) {
	created := time.Now()
	sessions := []*Session{
		{ID: "grok", CLI: config.GrokLabel, Model: config.GrokModel, Mode: "review", QueuedAt: &created},
		{ID: "sol", CLI: "codex", Model: config.GPT56SolModel, Mode: "review", QueuedAt: &created},
		{ID: "claude", CLI: "claude", Model: config.ClaudeModel, Mode: "review", QueuedAt: &created},
	}

	SortGroupMembers(sessions)

	want := []string{"sol", "claude", "grok"}
	for i, id := range want {
		if sessions[i].ID != id {
			t.Fatalf("sorted order = %s, want %s at index %d", sessions[i].ID, id, i)
		}
	}
}
