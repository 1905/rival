package skills

import (
	"slices"
	"strings"
	"testing"

	"gopkg.in/yaml.v3"
)

func TestSolSkillsAreRetired(t *testing.T) {
	for _, name := range []string{"rival-sol", "rival-plan-sol"} {
		if slices.Contains(Names, name) || !slices.Contains(Deprecated, name) {
			t.Fatalf("%s must be retired and cleaned on install", name)
		}
		if _, err := Files.ReadFile(name + "/SKILL.md"); err == nil {
			t.Fatalf("retired skill %s remains embedded", name)
		}
		if _, err := CodexSkill(name, "test"); err == nil {
			t.Fatalf("retired Codex skill %s remains available", name)
		}
	}
	for _, name := range Names {
		claude, err := Files.ReadFile(name + "/SKILL.md")
		if err != nil {
			t.Fatal(err)
		}
		codex, err := CodexSkill(name, "test")
		if err != nil {
			t.Fatal(err)
		}
		for _, text := range []string{string(claude), string(codex)} {
			for _, forbidden := range []string{"rival-sol", "rival-plan-sol", "Sol", "-m sol", "--model sol"} {
				if strings.Contains(text, forbidden) {
					t.Fatalf("%s still advertises %q", name, forbidden)
				}
			}
		}
	}
}

func TestEverySkillHasValidCodexVariant(t *testing.T) {
	for _, name := range Names {
		t.Run(name, func(t *testing.T) {
			data, err := CodexSkill(name, "9.8.7")
			if err != nil {
				t.Fatal(err)
			}
			parts := strings.SplitN(string(data), "---", 3)
			if len(parts) != 3 {
				t.Fatal("missing frontmatter")
			}
			var header struct {
				Name        string            `yaml:"name"`
				Description string            `yaml:"description"`
				Metadata    map[string]string `yaml:"metadata"`
			}
			if err := yaml.Unmarshal([]byte(parts[1]), &header); err != nil {
				t.Fatal(err)
			}
			if header.Name != name || header.Description == "" || header.Metadata["version"] != "9.8.7" {
				t.Fatalf("invalid header: %+v", header)
			}
			for _, unsupported := range []string{"$ARGUMENTS", "run_in_background", "Write tool", "{{COMMAND}}"} {
				if strings.Contains(string(data), unsupported) {
					t.Fatalf("unresolved or host-incompatible instruction %q", unsupported)
				}
			}
		})
	}
	if _, err := CodexSkill("rival-unknown", "1"); err == nil {
		t.Fatal("missing command accepted")
	}
}

// megareview was removed on 2026-09-26. Its skill must stay deprecated so
// rival install removes copies already on disk, for both hosts.
func TestReviewSkillIsRetired(t *testing.T) {
	const name = "rival-review"
	if slices.Contains(Names, name) || !slices.Contains(Deprecated, name) {
		t.Fatalf("%s must be retired and cleaned on install", name)
	}
	if _, err := Files.ReadFile(name + "/SKILL.md"); err == nil {
		t.Fatalf("retired skill %s remains embedded", name)
	}
	if _, err := CodexSkill(name, "test"); err == nil {
		t.Fatalf("retired Codex skill %s remains available", name)
	}
}
