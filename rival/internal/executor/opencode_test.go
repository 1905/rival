package executor

import (
	"os/exec"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

func TestOpencodePreflight_K3RequiresKey(t *testing.T) {
	// This branch only fires when the opencode CLI is installed; skip otherwise
	// so the test is deterministic across machines.
	if _, err := exec.LookPath("opencode"); err != nil {
		t.Skip("opencode CLI not installed")
	}

	t.Setenv("MOONSHOT_API_KEY", "")
	t.Setenv("KIMI_API", "")
	if err := OpencodePreflightModel(config.KimiModel, ""); err == nil {
		t.Fatal("expected preflight error when K3 has no MOONSHOT_API_KEY")
	} else if !strings.Contains(err.Error(), "MOONSHOT_API_KEY") {
		t.Errorf("preflight error should name the missing key, got: %v", err)
	}

	t.Setenv("MOONSHOT_API_KEY", "sk-test")
	if err := OpencodePreflightModel(config.KimiModel, ""); err != nil {
		t.Errorf("preflight should pass with a key set, got: %v", err)
	}
}

func TestOpencodePreflightRejectsUnsupportedModel(t *testing.T) {
	if err := OpencodePreflightModel("custom/example-model", ""); err == nil {
		t.Fatal("unsupported OpenCode model was accepted")
	}
}

func TestOpencodeProviderConfig(t *testing.T) {
	// An unregistered model has no entry at all.
	if _, ok := config.OpenCodeEntryFor("custom/example-model"); ok {
		t.Error("an unregistered model resolved to a registry entry")
	}
	entry, ok := config.OpenCodeEntryFor(config.KimiModel)
	if !ok {
		t.Fatal("K3 missing from the registry")
	}
	// Moonshot model → built-in provider "moonshotai".
	got := opencodeProviderConfig(entry, "sk-moon")
	if !strings.Contains(got, `"moonshotai"`) {
		t.Errorf("moonshot provider config wrong: %s", got)
	}
	// An empty key yields an empty config.
	if opencodeProviderConfig(entry, "") != "" {
		t.Error("empty key should yield empty config")
	}
}

func TestOpencodeRunArgs_UsesOnlySupportedVariants(t *testing.T) {
	k3, ok := config.OpenCodeEntryFor(config.KimiModel)
	if !ok {
		t.Fatal("K3 missing from the registry")
	}
	tests := []struct {
		name   string
		entry  config.SecurityModel
		effort string
		want   string
	}{
		{name: "kimi-k3 pins max", entry: k3, effort: "max", want: "--variant max"},
		{name: "kimi-k3 pins max at low", entry: k3, effort: "low", want: "--variant max"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			joined := strings.Join(opencodeRunArgs(tc.entry, tc.effort, "/repo"), " ")
			if !strings.Contains(joined, tc.want) {
				t.Fatalf("args %q do not contain %q", joined, tc.want)
			}
		})
	}
}

func TestOpencodeRunEnv_IsolatesSessionDatabases(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "sk-test")
	first := strings.Join(opencodeRunEnvWith("session-a", mustK3Entry(t), "", OpencodeRunOpts{}), "\n")
	second := strings.Join(opencodeRunEnvWith("session-b", mustK3Entry(t), "", OpencodeRunOpts{}), "\n")
	if !strings.Contains(first, "OPENCODE_DB=rival-session-a.db") {
		t.Fatalf("first env missing isolated DB: %s", first)
	}
	if !strings.Contains(second, "OPENCODE_DB=rival-session-b.db") {
		t.Fatalf("second env missing isolated DB: %s", second)
	}
	if first == second {
		t.Fatal("different sessions received identical OpenCode environments")
	}
}
