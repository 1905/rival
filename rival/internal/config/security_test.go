package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestResolveSecurityModelDefaultsToK3(t *testing.T) {
	userConfig = nil
	entry, err := ResolveSecurityModel()
	if err != nil {
		t.Fatalf("ResolveSecurityModel: %v", err)
	}
	if entry.Name != SecurityReviewerK3 || entry.Model != KimiModel {
		t.Errorf("default resolved to %+v, want k3", entry)
	}
	if entry.Variant != "max" {
		t.Errorf("k3 variant = %q, want max (its provider exposes no other level)", entry.Variant)
	}
}

func TestResolveSecurityModelGrok(t *testing.T) {
	userConfig = &UserConfig{Security: SecurityConfig{Reviewer: SecurityReviewerGrok}}
	defer func() { userConfig = nil }()

	entry, err := ResolveSecurityModel()
	if err != nil {
		t.Fatalf("ResolveSecurityModel: %v", err)
	}
	if entry.Model != GrokOpenRouterModel {
		t.Errorf("model = %q, want %q", entry.Model, GrokOpenRouterModel)
	}
	// OpenCode splits the -m value at the first slash to pick the provider, so
	// the selector must name openrouter or it would look for a provider
	// called "x-ai".
	if entry.Selector != GrokOpenRouterSelector {
		t.Errorf("selector = %q, want %q", entry.Selector, GrokOpenRouterSelector)
	}
	if entry.KeyEnv != "OPENROUTER_API_KEY" {
		t.Errorf("key env = %q", entry.KeyEnv)
	}
	if entry.Variant != "xhigh" {
		t.Errorf("variant = %q, want xhigh", entry.Variant)
	}
}

func TestResolveSecurityModelRejectsUnknown(t *testing.T) {
	userConfig = &UserConfig{Security: SecurityConfig{Reviewer: "gpt5"}}
	defer func() { userConfig = nil }()

	_, err := ResolveSecurityModel()
	if err == nil {
		t.Fatal("expected an unknown reviewer to error")
	}
	for _, want := range SecurityReviewerNames() {
		if !contains(err.Error(), want) {
			t.Errorf("error %q does not name the accepted value %q", err, want)
		}
	}
}

// The two Grok models must never share a public label, or their sessions and
// logs become indistinguishable.
func TestGrokLabelsDoNotCollide(t *testing.T) {
	if GrokOpenRouterLabel == GrokLabel {
		t.Fatal("the OpenRouter Grok label collides with the xAI Grok label")
	}
	if GrokOpenRouterLabel == GrokModel {
		t.Fatalf("the OpenRouter label %q collides with the xAI concrete model id", GrokOpenRouterLabel)
	}
}

// An explicit -m selection must not consult the security config.
func TestOpenCodeEntryForIgnoresConfig(t *testing.T) {
	userConfig = &UserConfig{Security: SecurityConfig{Reviewer: SecurityReviewerGrok}}
	defer func() { userConfig = nil }()

	entry, ok := OpenCodeEntryFor(KimiModel)
	if !ok {
		t.Fatal("K3 model not found in the registry")
	}
	if entry.KeyEnv != "MOONSHOT_API_KEY" {
		t.Errorf("key env = %q, want K3's own key despite security.reviewer=grok", entry.KeyEnv)
	}
	if _, ok := OpenCodeEntryFor("no-such-model"); ok {
		t.Error("an unknown model resolved to an entry")
	}
}

func contains(haystack, needle string) bool {
	return len(haystack) >= len(needle) && (haystack == needle ||
		len(needle) == 0 || indexOf(haystack, needle) >= 0)
}

func indexOf(h, n string) int {
	for i := 0; i+len(n) <= len(h); i++ {
		if h[i:i+len(n)] == n {
			return i
		}
	}
	return -1
}

// The xAI Grok bumped to 4.6 on 2026-08-20, after a live probe confirmed the
// CLI accepts it. That makes its concrete id textually close to the
// OpenRouter model's, so assert the labels stay distinct in every direction.
func TestBothGroksNormalizeToTheirOwnLabel(t *testing.T) {
	xaiLog := "runtime banner from " + GrokModel + " finished"
	if got := PublicRuntimeLog(GrokLabel, GrokModel, xaiLog); !contains(got, GrokLabel) {
		t.Errorf("xAI log did not normalize to %q: %s", GrokLabel, got)
	}

	orLog := "runtime banner from " + GrokOpenRouterModel + " finished"
	got := PublicRuntimeLog("opencode", GrokOpenRouterModel, orLog)
	if !contains(got, GrokOpenRouterLabel) {
		t.Errorf("OpenRouter log did not normalize to %q: %s", GrokOpenRouterLabel, got)
	}

	// The xAI model id must not appear as a bare label in OpenRouter output.
	if ModelLabel(GrokModel) == ModelLabel(GrokOpenRouterModel) {
		t.Errorf("both Groks resolve to the same label %q", ModelLabel(GrokModel))
	}
}

// K3 accepted the legacy KIMI_API alias from a project .env, not only from
// the process environment. The registry must not quietly drop that.
func TestK3KeyStillReadsTheLegacyEnvAlias(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "")
	t.Setenv("KIMI_API", "")

	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, ".env"), []byte("KIMI_API=from-dotenv\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	entry, ok := OpenCodeEntryFor(KimiModel)
	if !ok {
		t.Fatal("K3 missing from the registry")
	}
	if got := SecurityAPIKeyFrom(entry, dir); got != "from-dotenv" {
		t.Errorf("legacy .env alias not read: got %q", got)
	}
}

// Normalization must be idempotent. Logs get normalized more than once, and
// "grok-4.6-openrouter" contains the id "grok-4.6", so a careless pass
// rewrites an already-correct label into "grok-openrouter".
func TestRuntimeLogNormalizationIsIdempotent(t *testing.T) {
	cases := []struct {
		name  string
		cli   string
		model string
		raw   string
		want  string
	}{
		{"openrouter id becomes its label", "opencode", GrokOpenRouterModel,
			"banner from " + GrokOpenRouterModel + " done", GrokOpenRouterLabel},
		{"xai id becomes its label", GrokLabel, GrokModel,
			"banner from " + GrokModel + " done", GrokLabel},
		{"k3 id becomes its label", "opencode", KimiModel,
			"banner from " + KimiModel + " done", K3Label},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			once := PublicRuntimeLog(tc.cli, tc.model, tc.raw)
			if !contains(once, tc.want) {
				t.Fatalf("first pass = %q, want it to contain %q", once, tc.want)
			}
			twice := PublicRuntimeLog(tc.cli, tc.model, once)
			if twice != once {
				t.Errorf("not idempotent:\nonce:  %q\ntwice: %q", once, twice)
			}
		})
	}
}

// The Claude id gained a "-1" suffix on 2026-09-02. "claude-claude-5" is a
// prefix of "claude-opus-5-5", which is the same substring hazard that
// corrupted the Grok label, so pin both directions.
func TestClaudeIDNormalizesToItsLabel(t *testing.T) {
	raw := "banner from " + ClaudeModel + " done"
	once := PublicRuntimeLog("claude", ClaudeModel, raw)
	if !contains(once, ClaudeLabel) {
		t.Errorf("claude id not normalized: %q", once)
	}
	if contains(once, ClaudeModel) {
		t.Errorf("concrete claude id leaked: %q", once)
	}
	if twice := PublicRuntimeLog("claude", ClaudeModel, once); twice != once {
		t.Errorf("not idempotent:\nonce:  %q\ntwice: %q", once, twice)
	}
}
