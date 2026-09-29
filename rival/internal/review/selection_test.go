package review

import (
	"testing"

	"github.com/1905/rival/internal/config"
)

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
