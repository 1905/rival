package skills

import "embed"

//go:embed all:rival-codex
//go:embed all:rival-plan
//go:embed all:rival-plan-codex
//go:embed all:rival-plan-claude
//go:embed all:rival-claude
//go:embed all:rival-k3
//go:embed all:rival-grok
//go:embed all:rival-antislop
//go:embed all:rival-security
var Files embed.FS

// Names lists all embedded skill directory names.
var Names = []string{"rival-codex", "rival-plan", "rival-plan-codex", "rival-plan-claude", "rival-claude", "rival-k3", "rival-grok", "rival-antislop", "rival-security"}

// Deprecated lists legacy or superseded skills that should be removed on
// install. Re-enable a skill by adding it back to Names and the //go:embed list.
var Deprecated = []string{
	"rival-sol",
	"rival-plan-sol",
	"rival-claude-only",
	"rival-fable-only",
	"rival-codex-only",
	"rival-gpt-5-6-sol",
	"rival-claude-fable",
	"rival-astra",         // renamed to rival-codex in 3.34
	"rival-plan-astra",    // renamed to rival-plan-codex in 3.34
	"rival-fable",         // Fable retired in 3.34; rival-claude runs Opus 5.5
	"rival-plan-fable",    // Fable retired in 3.34; see rival-plan-claude
	"rival-kimi",          // renamed to rival-k3 before release
	"rival-antislop-plan", // plan mode dropped on 2026-08-20
	"rival-review",        // megareview removed 2026-09-26
}
