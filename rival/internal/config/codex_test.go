package config

import "testing"

// Codex and Sol share the codex adapter, so EngineLabel's `cli == "codex"`
// fallback would label every Codex run "sol" if the exact id were not matched
// first. This is the same collision that once labelled Grok as K3.
func TestCodexIsNotLabelledSol(t *testing.T) {
	if got := EngineLabel("codex", CodexModel); got != CodexLabel {
		t.Errorf("EngineLabel(codex, codex) = %q, want %q", got, CodexLabel)
	}
	if got := EngineLabel("codex", GPT56SolModel); got != SolLabel {
		t.Errorf("EngineLabel(codex, sol) = %q, want %q", got, SolLabel)
	}
	if got := ModelLabel(CodexModel); got != CodexLabel {
		t.Errorf("ModelLabel(codex) = %q, want %q", got, CodexLabel)
	}
}

// The concrete id must never reach a public log, and normalization must be
// idempotent.
func TestCodexIDNormalizesToItsLabel(t *testing.T) {
	raw := "banner from " + CodexModel + " done"
	once := PublicRuntimeLog("codex", CodexModel, raw)
	if contains(once, CodexModel) {
		t.Errorf("concrete codex id leaked: %q", once)
	}
	if !contains(once, CodexLabel) {
		t.Errorf("codex not normalized: %q", once)
	}
	if twice := PublicRuntimeLog("codex", CodexModel, once); twice != once {
		t.Errorf("not idempotent:\nonce:  %q\ntwice: %q", once, twice)
	}
}

// The user asked for xhigh, and an unknown effort label would be rejected by
// config validation.
func TestCodexDefaultsToXhigh(t *testing.T) {
	if got := DefaultEffortForModel(CodexModel); got != "xhigh" {
		t.Errorf("default effort = %q, want xhigh", got)
	}
	if !knownEffortModel(CodexLabel) {
		t.Error("efforts.codex would be rejected as an unknown model")
	}
}

// The command path passes its own fallback, which short-circuits
// builtinModelEffort — the direct DefaultEffortForModel test above does not
// cover it, and an early build regressed here.
func TestCodexCommandPathKeepsXhigh(t *testing.T) {
	got, err := ResolveEffort(CodexModel, "", "")
	if err != nil {
		t.Fatalf("ResolveEffort: %v", err)
	}
	if got != "xhigh" {
		t.Errorf("command-path effort = %q, want xhigh", got)
	}
}

// -m codex must select the codex adapter with Codex's concrete id.
func TestCodexSelectableInMegareview(t *testing.T) {
	targets, err := ResolveReviewTargets([]string{"codex"})
	if err != nil {
		t.Fatalf("ResolveReviewTargets: %v", err)
	}
	if len(targets) != 1 || targets[0].Model != CodexModel || targets[0].CLI != "codex" {
		t.Fatalf("got %+v", targets)
	}
	if targets[0].Prompt != PromptBugHunter {
		t.Errorf("codex should run the bug-hunter lens, got %v", targets[0].Prompt)
	}
}

// The pin must not outrank an explicit override or user config — silently
// ignoring what the user asked for would be worse than the bug it fixes.
func TestCodexPinDoesNotOverrideUserIntent(t *testing.T) {
	if got, _ := ResolveEffort(CodexModel, "medium", "high"); got != "medium" {
		t.Errorf("explicit -re ignored: got %q, want medium", got)
	}

	prev := userConfig
	t.Cleanup(func() { userConfig = prev })
	userConfig = &UserConfig{Efforts: map[string]string{CodexLabel: "high"}}
	if got, _ := ResolveEffort(CodexModel, "", "medium"); got != "high" {
		t.Errorf("user config ignored: got %q, want high", got)
	}
}

// Every surface must reach xhigh, not just the single-model command. The
// megareview and plan paths each pass their own non-empty fallback.
func TestCodexXhighOnEverySurface(t *testing.T) {
	for _, fallback := range []string{"", DefaultReviewEffort, DefaultPlanEffort} {
		got, err := ResolveEffort(CodexModel, "", fallback)
		if err != nil {
			t.Fatalf("ResolveEffort(fallback=%q): %v", fallback, err)
		}
		if got != "xhigh" {
			t.Errorf("fallback %q gave effort %q, want xhigh", fallback, got)
		}
	}
}

// Sol and Codex share the codex runtime, so its banner and preflight errors
// must name the model that actually ran.
func TestCodexBannerNamesTheRunningModel(t *testing.T) {
	const raw = "OpenAI Codex v1.0\nready"
	sol := PublicRuntimeLog("codex", GPT56SolModel, raw)
	if !contains(sol, "Sol runtime") {
		t.Errorf("sol banner regressed: %q", sol)
	}
	codex := PublicRuntimeLog("codex", CodexModel, raw)
	if !contains(codex, "Codex runtime") {
		t.Errorf("codex banner still says sol: %q", codex)
	}
	if contains(codex, "OpenAI Codex") {
		t.Errorf("raw runtime name leaked: %q", codex)
	}
}
