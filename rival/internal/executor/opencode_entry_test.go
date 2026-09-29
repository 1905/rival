package executor

import (
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

func k3Entry(t *testing.T) config.SecurityModel {
	t.Helper()
	entry, ok := config.OpenCodeEntryFor(config.KimiModel)
	if !ok {
		t.Fatal("K3 missing from the registry")
	}
	return entry
}

func grokEntry(t *testing.T) config.SecurityModel {
	t.Helper()
	entry, ok := config.OpenCodeEntryFor(config.GrokOpenRouterModel)
	if !ok {
		t.Fatal("OpenRouter Grok missing from the registry")
	}
	return entry
}

// Captured from the tree before the registry existed. Generalizing the
// adapter must not change one byte of what K3 already sends.
const k3ProviderBaseline = `{"$schema":"https://opencode.ai/config.json","provider":{"moonshotai":{"options":{"apiKey":"TESTKEY"}}}}`

func TestK3ProviderConfigUnchanged(t *testing.T) {
	got := opencodeProviderConfig(k3Entry(t), "TESTKEY")
	if got != k3ProviderBaseline {
		t.Errorf("K3 provider config changed.\ngot:  %s\nwant: %s", got, k3ProviderBaseline)
	}
}

func TestGrokProviderConfigNamesOpenRouter(t *testing.T) {
	got := opencodeProviderConfig(grokEntry(t), "TESTKEY")
	if !strings.Contains(got, `"openrouter"`) {
		t.Errorf("provider block does not name openrouter: %s", got)
	}
	if !strings.Contains(got, "https://openrouter.ai/api/v1") {
		t.Errorf("provider block missing the OpenRouter base URL: %s", got)
	}
	if strings.Contains(got, "moonshotai") {
		t.Errorf("Grok config leaked the Moonshot provider: %s", got)
	}
}

func TestProviderConfigRejectsEmptyKey(t *testing.T) {
	if got := opencodeProviderConfig(k3Entry(t), ""); got != "" {
		t.Errorf("empty key produced a config: %s", got)
	}
}

// OpenCode splits the -m value at the first slash to choose a provider, so
// passing the upstream id would look for a provider named "x-ai".
func TestRunArgsUseTheSelectorNotTheModelID(t *testing.T) {
	args := strings.Join(opencodeRunArgs(grokEntry(t), "xhigh", "/tmp"), " ")
	if !strings.Contains(args, "-m "+config.GrokOpenRouterSelector) {
		t.Errorf("args do not pass the OpenRouter selector: %s", args)
	}
	if strings.Contains(args, "-m "+config.GrokOpenRouterModel+" ") {
		t.Errorf("args passed the upstream model id, which selects the wrong provider: %s", args)
	}
	if !strings.Contains(args, "--variant xhigh") {
		t.Errorf("Grok's pinned variant is missing: %s", args)
	}
}

func TestK3RunArgsKeepMaxVariant(t *testing.T) {
	args := strings.Join(opencodeRunArgs(k3Entry(t), "high", "/tmp"), " ")
	if !strings.Contains(args, "-m "+config.KimiModel) {
		t.Errorf("K3 selector changed: %s", args)
	}
	// K3's provider exposes only max, whatever effort was requested.
	if !strings.Contains(args, "--variant max") {
		t.Errorf("K3 lost its max pin: %s", args)
	}
}

func TestPreflightNamesTheRightVariablePerModel(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "")
	t.Setenv("KIMI_API", "")
	t.Setenv("OPENROUTER_API_KEY", "")

	for _, tc := range []struct {
		entry config.SecurityModel
		want  string
	}{
		{k3Entry(t), "MOONSHOT_API_KEY"},
		{grokEntry(t), "OPENROUTER_API_KEY"},
	} {
		err := OpencodePreflightEntry(tc.entry, t.TempDir())
		if err == nil {
			t.Fatalf("%s: expected a missing-key error", tc.entry.Name)
		}
		// A missing binary is a different failure; only assert the key case.
		if strings.Contains(err.Error(), "not installed") {
			continue
		}
		if !strings.Contains(err.Error(), tc.want) {
			t.Errorf("%s error names the wrong variable: %v", tc.entry.Name, err)
		}
	}
}

// mustK3Entry is the registry lookup the migrated tests share.
func mustK3Entry(t *testing.T) config.SecurityModel {
	t.Helper()
	return k3Entry(t)
}
