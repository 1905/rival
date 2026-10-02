package skills

import (
	"slices"
	"strings"
	"testing"
)

func TestPlanCodexSkillPinsXhighEffort(t *testing.T) {
	data, err := Files.ReadFile("rival-plan-codex/SKILL.md")
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	for _, want := range []string{
		"version: ",
		"argument-hint: \"<path-to-plan.md>\"",
		"rival command plan --model codex --effort xhigh --detach",
		"Always run at **xhigh**",
	} {
		if !strings.Contains(content, want) {
			t.Errorf("plan-codex skill missing %q", want)
		}
	}
	for _, forbidden := range []string{"defaults to **high**", "[-re high|ultra]"} {
		if strings.Contains(content, forbidden) {
			t.Errorf("plan-codex skill still advertises optional effort %q", forbidden)
		}
	}
}

func TestGrokSkillIsEmbedded(t *testing.T) {
	const name = "rival-grok"
	if !slices.Contains(Names, name) {
		t.Fatalf("grok skill %q is not active", name)
	}
	if slices.Contains(Deprecated, name) {
		t.Fatalf("grok skill %q is deprecated", name)
	}
	data, err := Files.ReadFile(name + "/SKILL.md")
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	for _, want := range []string{
		"version: ",
		"name: rival-grok",
		"rival command grok --detach --workdir",
		"rival wait --log <rival_err>",
		"grok login",
	} {
		if !strings.Contains(content, want) {
			t.Errorf("grok skill missing %q", want)
		}
	}
	// A hardcoded watcher timeout re-introduces the bound that
	// RIVAL_QUEUE_TIMEOUT/RIVAL_RUN_TIMEOUT already own.
	if strings.Contains(content, "rival wait --log <rival_err> --timeout") {
		t.Error("grok skill hardcodes a --timeout on the watcher")
	}
}

func TestPlanSkillRunsCodexAtXhigh(t *testing.T) {
	const name = "rival-plan"
	if !slices.Contains(Names, name) {
		t.Fatalf("paired plan skill %q is not active", name)
	}
	if slices.Contains(Deprecated, name) {
		t.Fatalf("paired plan skill %q is still deprecated", name)
	}
	data, err := Files.ReadFile(name + "/SKILL.md")
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	for _, want := range []string{
		"name: rival-plan",
		"Codex",
		"rival command plan --model codex --effort xhigh --detach",
	} {
		if !strings.Contains(content, want) {
			t.Errorf("paired plan skill missing %q", want)
		}
	}
}

// Plan mode was dropped on 2026-08-20. The skill must stay deprecated so
// rival install removes copies that are already on disk.
func TestAntislopPlanSkillIsGone(t *testing.T) {
	if slices.Contains(Names, "rival-antislop-plan") {
		t.Error("rival-antislop-plan is active again; plan mode was removed")
	}
	if !slices.Contains(Deprecated, "rival-antislop-plan") {
		t.Error("rival-antislop-plan must stay in Deprecated so installs clean it up")
	}
	if _, err := Files.ReadFile("rival-antislop-plan/SKILL.md"); err == nil {
		t.Error("the plan skill file is still embedded")
	}
}

func TestAntislopSkillsAreEmbedded(t *testing.T) {
	tests := []struct {
		name  string
		wants []string
	}{
		{"rival-antislop", []string{
			"name: rival-antislop\n",
			"argument-hint: \"[<scope>]\"",
			"rival command antislop --detach --workdir",
			"rival wait --log <rival_err>",
			"never bugs",
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if !slices.Contains(Names, tt.name) {
				t.Fatalf("skill %q is not active", tt.name)
			}
			if slices.Contains(Deprecated, tt.name) {
				t.Fatalf("skill %q is deprecated", tt.name)
			}
			data, err := Files.ReadFile(tt.name + "/SKILL.md")
			if err != nil {
				t.Fatal(err)
			}
			content := string(data)
			for _, want := range append(tt.wants, "version: ") {
				if !strings.Contains(content, want) {
					t.Errorf("%s skill missing %q", tt.name, want)
				}
			}
		})
	}
}

func TestSkillWatchersUseHeartbeat(t *testing.T) {
	// Every watcher wakes its caller periodically so a long run reports
	// progress and a dead watcher is noticed instead of waited on silently.
	for _, name := range Names {
		data, err := Files.ReadFile(name + "/SKILL.md")
		if err != nil {
			t.Fatal(err)
		}
		content := string(data)
		for _, want := range []string{"rival wait --log <rival_err> --heartbeat 5m", "### Heartbeat", "`5` heartbeat"} {
			if !strings.Contains(content, want) {
				t.Errorf("%s skill missing %q", name, want)
			}
		}
	}
}
