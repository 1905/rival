package skills

import (
	_ "embed"
	"fmt"
	"strings"
)

//go:embed codex.md
var codexWorkflow string

// CodexSkill shares the command parsers, model defaults, and embedded release
// version with Claude's skills, but uses Codex's process lifecycle.
func CodexSkill(name, version string) ([]byte, error) {
	var description, input, command string
	switch name {
	case "rival-claude":
		description = "Review code with Opus 5.5 through Rival and the authenticated Claude Code CLI. Use for a requested independent Claude review from Codex."
		command = "claude"
		input = "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Claude default (medium fallback). For a plan document use $rival-plan-claude. Requires the Claude Code CLI authenticated with `claude auth login`, or Rival's configured Docker transport. Rival selects Opus 5.5; do not replace it with another model."
	case "rival-codex", "rival-k3", "rival-grok":
		model := strings.TrimPrefix(name, "rival-")
		description = fmt.Sprintf("Run a requested %s prompt or code review through Rival from Codex.", model)
		command = model
		input = "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Model defaults and provider setup are owned by Rival; do not invent flags or substitute another model."
	case "rival-review":
		description = "Run Rival's independent code review and consilium from Codex. Use for a requested Rival review; use rival-claude for a Claude-only review."
		command = "megareview"
		input = "Pass the user's scope and options verbatim. Empty input reviews git-detected changes. The default reviewer is Codex. `-m codex,k3` selects two reviewers; Grok is opt-in. Claude is not supported in this roster: use $rival-claude for Claude. For a GitLab MR, pass its HTTPS URL as the entire scope and use a local repository with a remote for the target project as the workdir. The host needs glab authentication for that host and Git fetch access. Rival resolves the base/head SHAs and reviews an isolated checkout. If resolution fails, report the failure; never retry against local HEAD. Preserve the URL/base/head header in the report. Do not post MR comments or approvals."
	case "rival-plan", "rival-plan-codex", "rival-plan-claude":
		description = "Review a plan or specification document through Rival from Codex, returning ratings and findings."
		command = "plan --model codex --effort xhigh"
		switch name {
		case "rival-plan-codex":
			command = "plan --model codex --effort xhigh"
		case "rival-plan-claude":
			command = "plan --model claude"
		}
		input = "Pass the document path and any requested options verbatim. If no document is specified, ask for its path before launching. Show all model results and report any skipped model. Codex plan reviews pin xhigh; Claude-only uses its configured effort (medium fallback) unless the user supplies -re."
	case "rival-antislop":
		description = "Review code for over-engineering and unnecessary complexity through Rival, returning a leanness rating and cut list. Use for requested antislop reviews."
		command = "antislop"
		input = "Pass the scope and options verbatim. Empty input reviews git-detected changes. Default Codex, high fallback; `-m claude` selects Claude. This reports quality and simplification findings, not ordinary bug findings."
	case "rival-security":
		description = "Run Rival's dedicated security reviewer on changed code or a specified scope from Codex. Use for requested vulnerability reviews."
		command = "security"
		input = "First run `rival command security --which --workdir <absolute-repository>` and report the resolved model. If it fails, report the error and do not launch. Pass the user's scope verbatim; empty input reviews git-detected changes. The model is selected by security.reviewer in Rival's configuration."
	default:
		return nil, fmt.Errorf("no Codex skill for %q", name)
	}
	content := fmt.Sprintf("---\nname: %s\ndescription: %s\nmetadata:\n  version: %s\n---\n\n# %s\n\n## Review input\n\n%s\n\n", name, description, version, name, input)
	content += strings.ReplaceAll(codexWorkflow, "{{COMMAND}}", command)
	return []byte(content), nil
}
