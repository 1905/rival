package cmd

import (
	"strings"
	"testing"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

func TestReviewCommandsExposeModelFlag(t *testing.T) {
	for _, tc := range []struct {
		name  string
		flags *pflag.FlagSet
	}{
		{name: "rival review", flags: reviewCmd.Flags()},
		{name: "rival command megareview", flags: commandMegareviewCmd.Flags()},
	} {
		flag := tc.flags.Lookup("model")
		if flag == nil {
			t.Fatalf("%s has no --model flag", tc.name)
		}
		if flag.Shorthand != "m" {
			t.Errorf("%s --model shorthand = %q, want m", tc.name, flag.Shorthand)
		}
	}
}

func TestReviewCommandDefersToConfiguredModelEfforts(t *testing.T) {
	flag := reviewCmd.Flags().Lookup("effort")
	if flag == nil {
		t.Fatal("review command has no --effort flag")
	}
	if flag.DefValue != "" {
		t.Fatalf("review effort default = %q, want configured-model sentinel", flag.DefValue)
	}
}

func TestMegareviewUsageNamesModelsAndEffortConfig(t *testing.T) {
	for _, want := range []string{"Codex", "Kimi K3", "model defaults", "ultra", "grok"} {
		if !strings.Contains(megareviewUsage, want) {
			t.Errorf("megareview usage missing %q", want)
		}
	}
	if strings.Contains(strings.ToLower(megareviewUsage), "gpt-5.6") {
		t.Fatal("megareview usage advertises an internal model id")
	}
}

// grok is selectable but opt-in, so both the --model help text and the
// empty-value error must name it alongside the default roster.
func TestReviewModelFlagHelpNamesGrok(t *testing.T) {
	flag := reviewCmd.Flags().Lookup("model")
	if flag == nil {
		t.Fatal("review command has no --model flag")
	}
	for _, want := range []string{"sol", "kimi-k3", "grok"} {
		if !strings.Contains(flag.Usage, want) {
			t.Errorf("--model help %q missing %q", flag.Usage, want)
		}
	}
}

func TestModelSelectionFlag_EmptyErrorNamesGrok(t *testing.T) {
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringSliceP("model", "m", nil, "models")
	if err := cmd.ParseFlags([]string{"--model="}); err != nil {
		t.Fatal(err)
	}
	_, _, err := modelSelectionFlag(cmd)
	if err == nil {
		t.Fatal("expected an empty --model error")
	}
	if !strings.Contains(err.Error(), "grok") {
		t.Errorf("empty --model error %q does not mention grok", err)
	}
}

func TestModelSelectionFlag_ParsesGrok(t *testing.T) {
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringSliceP("model", "m", nil, "models")
	if err := cmd.ParseFlags([]string{"--model", "grok,sol"}); err != nil {
		t.Fatal(err)
	}
	models, changed, err := modelSelectionFlag(cmd)
	if err != nil {
		t.Fatal(err)
	}
	if !changed || len(models) != 2 || models[0] != "grok" || models[1] != "sol" {
		t.Fatalf("unexpected selection: changed=%v models=%v", changed, models)
	}
}

func TestModelSelectionFlag_RejectsExplicitEmpty(t *testing.T) {
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringSliceP("model", "m", nil, "models")
	if err := cmd.ParseFlags([]string{"--model="}); err != nil {
		t.Fatal(err)
	}
	_, changed, err := modelSelectionFlag(cmd)
	if !changed {
		t.Fatal("explicit --model= was not detected as changed")
	}
	if err == nil || !strings.Contains(err.Error(), "requires a value") {
		t.Fatalf("expected actionable empty-model error, got %v", err)
	}
}

func TestModelSelectionFlag_ParsesCommaList(t *testing.T) {
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringSliceP("model", "m", nil, "models")
	if err := cmd.ParseFlags([]string{"-m", "sol,k3"}); err != nil {
		t.Fatal(err)
	}
	models, changed, err := modelSelectionFlag(cmd)
	if err != nil {
		t.Fatal(err)
	}
	if !changed || len(models) != 2 || models[0] != "sol" || models[1] != "k3" {
		t.Fatalf("unexpected selection: changed=%v models=%v", changed, models)
	}
}
