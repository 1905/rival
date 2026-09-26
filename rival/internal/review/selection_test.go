package review

// These cases moved here when role.go was deleted. They test functions that
// survived the role removal: model derivation, judge selection, and the K3
// reasoning-variant pin.

import (
	"testing"

	"github.com/1F47E/rival/internal/config"
)

func TestModelForCLI_Opencode(t *testing.T) {
	if got := modelForCLI("opencode"); got != config.KimiModel {
		t.Errorf("modelForCLI(opencode) = %q, want %q", got, config.KimiModel)
	}
}

func TestModelForCLI_Grok(t *testing.T) {
	if got := modelForCLI(config.GrokLabel); got != config.GrokModel {
		t.Errorf("modelForCLI(grok) = %q, want %q", got, config.GrokModel)
	}
}

func TestPickJudge(t *testing.T) {
	cases := []struct {
		name      string
		inputs    []ReviewInput
		targets   []config.ReviewTarget
		wantCLI   string
		wantModel string
	}{
		{
			"default judge picks Codex regardless of completion order",
			[]ReviewInput{
				{CLI: "opencode", Model: config.KimiModel},
				{CLI: "codex", Model: config.CodexModel},
			},
			config.DefaultReviewTargets(),
			"codex", config.CodexModel,
		},
		{
			"requested order can select K3 before Sol",
			[]ReviewInput{
				{CLI: "codex", Model: config.GPT56SolModel},
				{CLI: "opencode", Model: config.KimiModel},
			},
			[]config.ReviewTarget{
				{CLI: "opencode", Model: config.KimiModel},
				{CLI: "codex", Model: config.GPT56SolModel},
			},
			"opencode", config.KimiModel,
		},
		{
			"default judge falls through to K3 when GPT-5.6-Sol failed",
			[]ReviewInput{{CLI: "opencode", Model: config.KimiModel}},
			config.DefaultReviewTargets(),
			"opencode", config.KimiModel,
		},
		{
			"single GPT-5.6-Sol success judges itself",
			[]ReviewInput{{CLI: "codex", Model: config.GPT56SolModel}},
			[]config.ReviewTarget{{CLI: "codex", Model: config.GPT56SolModel}},
			"codex", config.GPT56SolModel,
		},
		{
			"empty on no successful inputs",
			nil,
			config.DefaultReviewTargets(),
			"", "",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			gotCLI, gotModel := pickJudge(tc.inputs, tc.targets)
			if gotCLI != tc.wantCLI {
				t.Errorf("pickJudge(%v) cli = %q, want %q", tc.inputs, gotCLI, tc.wantCLI)
			}
			if gotModel != tc.wantModel {
				t.Errorf("pickJudge(%v) model = %q, want %q", tc.inputs, gotModel, tc.wantModel)
			}
		})
	}
}

func TestOpencodeVariant_PerCuratedModel(t *testing.T) {
	cases := []struct{ model, effort, want string }{
		{config.KimiModel, "low", "max"},
		{config.KimiModel, "xhigh", "max"},
		{config.KimiModel, "ultra", "max"},
		{"unsupported-model", "high", ""},
	}
	for _, tc := range cases {
		if got := config.OpencodeVariant(tc.model, tc.effort); got != tc.want {
			t.Errorf("OpencodeVariant(%q, %q) = %q, want %q", tc.model, tc.effort, got, tc.want)
		}
	}
}
