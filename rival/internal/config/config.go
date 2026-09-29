package config

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/joho/godotenv"
	"gopkg.in/yaml.v3"
)

const (
	// GPT56SolModel and SolLabel name a removed model (2026-09-26). Nothing
	// runs it; the executor rejects it.
	GPT56SolModel = "gpt-5.6-sol" // read-compat: display of sessions recorded before Sol's removal
	// CodexModel shares the codex runtime with the removed Sol model, so
	// EngineLabel must match it before the "codex" adapter fallback below,
	// which labels old sessions "sol".
	// The public label equals the adapter name, so identity checks below
	// key on the model id, never on the bare word "codex".
	CodexModel  = "gpt-6-astra"
	CodexLabel  = "codex"
	ClaudeModel = "claude-opus-5-5"
	SolLabel    = "sol" // read-compat: display of sessions recorded before Sol's removal
	ClaudeLabel = "claude"
	K3Label     = "kimi-k3"
	// K3CommandName is the cobra command word for K3. It differs from
	// K3Label, which is the public display and error name.
	K3CommandName        = "k3"
	KimiModel            = "moonshotai/kimi-k3" // Kimi K3 via OpenCode's built-in Moonshot AI provider
	GrokModel            = "grok-4.6"
	GrokLabel            = "grok"
	ClaudeDockerImage    = "rival-claude"
	ClaudeDockerTokenEnv = "RIVAL_CLAUDE_TOKEN"

	DefaultReviewEffort   = "high"
	DefaultPlanEffort     = "high"
	DefaultAntislopEffort = "high"
	SessionDir            = ".rival/sessions"
	QueueDir              = ".rival/queue"
	PromptPreviewLen      = 100
	PromptDetailMaxLines  = 10

	DefaultMaxConcurrent = 2
	DefaultQueueTimeout  = 30 * time.Minute
	DefaultRunTimeout    = 30 * time.Minute
	QueuePollInterval    = 2 * time.Second
)

// ValidEfforts is the one effort ladder every surface accepts and advertises.
//
// xhigh and ultra are NOT interchangeable: the codex runtime passes the value
// through verbatim and treats ultra as its own reasoning level, so neither
// may be aliased to the other. Runtimes that expose a shorter menu clamp at
// their own boundary (see ClaudeEffortLevel and GrokEffort).
var ValidEfforts = []string{"low", "medium", "high", "xhigh", "ultra"}

// ClaudeEffortLevel maps rival effort levels to claude CLI --effort values.
var ClaudeEffortLevel = map[string]string{
	"low":    "low",
	"medium": "medium",
	"high":   "max",
	"xhigh":  "max",
	"ultra":  "max",
}

// OpencodeVariant returns K3's only provider-supported reasoning variant.
// Unknown models have no supported variant because K3 is Rival's sole
// OpenCode-backed model.
func OpencodeVariant(model, _ string) string {
	if model != KimiModel {
		return ""
	}
	return "max"
}

// ModelLabel returns the stable public name for a concrete model id. Runtime
// model ids stay internal so dashboards, console output, and API summaries use
// Rival's short model names consistently.
func ModelLabel(model string) string {
	switch model {
	case GPT56SolModel, SolLabel: // read-compat: display of sessions recorded before Sol's removal
		return SolLabel
	case CodexModel, CodexLabel:
		return CodexLabel
	case ClaudeModel, ClaudeLabel:
		return ClaudeLabel
	case KimiModel, K3Label:
		return K3Label
	case GrokModel, GrokLabel:
		return GrokLabel
	case GrokOpenRouterModel, GrokOpenRouterLabel:
		// Distinct from GrokLabel: this is Grok on OpenCode via OpenRouter,
		// a different runtime with a different credential.
		return GrokOpenRouterLabel
	default:
		return "retired-model"
	}
}

// EngineLabel returns a human-facing reviewer label. Review output names the
// selected model instead of the executable adapter used to launch it.
func EngineLabel(cli, model string) string {
	// Exact current ids win first.
	switch model {
	case GPT56SolModel: // read-compat: display of sessions recorded before Sol's removal
		return SolLabel
	case CodexModel:
		// Checked before the adapter fallback: Codex and the removed Sol both
		// ran on codex, so falling through would label Codex as Sol.
		return CodexLabel
	case ClaudeModel:
		return ClaudeLabel
	case KimiModel:
		return K3Label
	case GrokModel:
		return GrokLabel
	case GrokOpenRouterModel:
		// Checked before the adapter fallback below: both this and K3 run on
		// opencode, so falling through would label Grok as K3.
		return GrokOpenRouterLabel
	}

	// Adapter identity is the reliable fallback for sessions written by older
	// releases with now-obsolete model ids.
	switch cli {
	case "codex": // read-compat: display of sessions recorded before Sol's removal
		return SolLabel
	case GrokLabel:
		return GrokLabel
	case "claude", "fable", "opencode":
		return "retired-model"
	}
	if model != "" {
		return ModelLabel(model)
	}
	return cli
}

// PublicRuntimeError removes internal adapter and concrete model identifiers
// from an error before it is shown to a user. Required executable paths and
// configuration keys remain untouched.
func PublicRuntimeError(cli, model, message string) string {
	message = replaceConcreteModelIDs(cli, model, message)
	label := EngineLabel(cli, model)
	switch cli {
	case "codex", "astra":
		// "astra" is read-compat for plan sessions written before 3.34.
		title := titleLabel(label)
		return strings.NewReplacer(
			"OpenAI Codex", title+" runtime",
			"Codex CLI", title+" runtime",
			"codex CLI", title+" runtime",
			"run codex login", "authenticate the "+title+" runtime",
			"codex exited", label+" exited",
			"start codex:", "start "+title+" runtime:",
			"subprocess codex:", title+" runtime:",
			"Codex", title,
		).Replace(message)
	case "claude", "fable":
		// "fable" is read-compat for plan sessions written before 3.34.
		title := strings.ToUpper(label[:1]) + label[1:]
		return strings.NewReplacer(
			"Claude Code CLI", title+" runtime",
			"Claude CLI", title+" runtime",
			"claude CLI", label+" runtime",
			"claude requires Docker", title+" runtime requires Docker",
			"claude exited", label+" exited",
			"start claude:", "start "+title+" runtime:",
			"subprocess claude:", title+" runtime:",
		).Replace(message)
	default:
		return message
	}
}

// PublicRuntimeLog normalizes runtime banners and concrete model ids while
// preserving model output, including any source paths that contain an adapter
// name. Persisted logs stay lossless; every user-facing log reader calls this.
func PublicRuntimeLog(cli, model, raw string) string {
	if raw == "" {
		return raw
	}
	raw = replaceConcreteModelIDs(cli, model, raw)
	label := EngineLabel(cli, model)
	title := label
	if title != "" {
		title = strings.ToUpper(title[:1]) + title[1:]
	}

	lines := strings.SplitAfter(raw, "\n")
	bannerSeen := false
	headerOpen := true
	delimiters := 0
	for i, line := range lines {
		ending := ""
		body := line
		if strings.HasSuffix(body, "\n") {
			body = strings.TrimSuffix(body, "\n")
			ending = "\n"
		}
		trimmed := strings.TrimSpace(body)
		leading := body[:len(body)-len(strings.TrimLeft(body, " \t"))]

		switch cli {
		case "codex", "astra":
			if strings.HasPrefix(trimmed, "OpenAI Codex") {
				// Codex and old Sol sessions share this runtime, so the banner takes the
				// resolved label rather than a hardcoded "Sol". Title-cased to
				// match the display form the banner has always used.
				trimmed = titleLabel(EngineLabel(cli, model)) + " runtime" + strings.TrimPrefix(trimmed, "OpenAI Codex")
				body = leading + trimmed
				bannerSeen = true
			} else if i == 0 && strings.HasPrefix(trimmed, "Codex ") {
				// read-compat: display of sessions recorded before Sol's removal
				trimmed = "Sol runtime " + strings.TrimPrefix(trimmed, "Codex ")
				body = leading + trimmed
				bannerSeen = true
			}
		case "claude", "fable":
			if strings.HasPrefix(trimmed, "Claude Code") {
				trimmed = title + " runtime" + strings.TrimPrefix(trimmed, "Claude Code")
				body = leading + trimmed
				bannerSeen = true
			} else if i == 0 && strings.HasPrefix(trimmed, "Claude ") {
				trimmed = title + " runtime " + strings.TrimPrefix(trimmed, "Claude ")
				body = leading + trimmed
				bannerSeen = true
			}
		}

		if bannerSeen && headerOpen && strings.HasPrefix(strings.ToLower(trimmed), "model:") {
			body = leading + "model: " + label
		}
		if strings.HasPrefix(trimmed, "=== REVIEW FROM ") {
			body = leading + publicReviewHeader(trimmed)
		}

		if trimmed == "--------" {
			delimiters++
			if delimiters >= 2 {
				headerOpen = false
			}
		}
		if strings.EqualFold(trimmed, "user") {
			headerOpen = false
		}
		lines[i] = body + ending
	}
	return strings.Join(lines, "")
}

func replaceConcreteModelIDs(cli, model, text string) string {
	// Model ids overlap textually: "grok-4.6" is a substring of the label
	// "grok-4.6-openrouter". Replacing ids directly lets one substitution
	// corrupt another's output, so each id becomes a placeholder first and
	// only expands to its label once every id is consumed.
	type pair struct{ id, label string }
	pairs := []pair{
		{GPT56SolModel, SolLabel}, // read-compat: display of sessions recorded before Sol's removal
		{CodexModel, CodexLabel},
		{ClaudeModel, ClaudeLabel},
		{KimiModel, K3Label},
		{GrokOpenRouterModel, GrokOpenRouterLabel},
		{GrokModel, GrokLabel},
	}
	if model != "" {
		// The run's own model wins, and is matched before the shared list so
		// a longer id is never shadowed by a shorter one it contains.
		pairs = append([]pair{{model, EngineLabel(cli, model)}}, pairs...)
	}

	// Longest id first: a short id must never consume part of a longer one.
	sort.SliceStable(pairs, func(i, j int) bool {
		return len(pairs[i].id) > len(pairs[j].id)
	})

	labels := make([]string, 0, len(pairs))

	// Protect public labels before touching ids. Text can already contain a
	// label — a re-normalized log, or a model naming itself — and
	// "grok-4.6-openrouter" contains the id "grok-4.6", so an unprotected
	// label would be rewritten into "grok-openrouter".
	// SolLabel stays protected — read-compat: display of sessions recorded before Sol's removal.
	protected := []string{GrokOpenRouterLabel, K3Label, SolLabel, ClaudeLabel, GrokLabel, CodexLabel}
	sort.SliceStable(protected, func(i, j int) bool {
		return len(protected[i]) > len(protected[j])
	})
	for i, label := range protected {
		if !strings.Contains(text, label) {
			continue
		}
		// Skip a label that only appears inside a concrete id we are about to
		// replace. Protecting it there would mask the id and leave it
		// un-normalized.
		masksAnID := false
		for _, p := range pairs {
			if p.id != "" && strings.Contains(p.id, label) && strings.Contains(text, p.id) {
				masksAnID = true
				break
			}
		}
		if masksAnID {
			continue
		}
		token := "\x00rival-label-" + strconv.Itoa(i) + "\x00"
		text = strings.ReplaceAll(text, label, token)
		labels = append(labels, token, label)
	}

	for i, p := range pairs {
		if p.id == "" || !strings.Contains(text, p.id) {
			continue
		}
		token := "\x00rival-model-" + strconv.Itoa(i) + "\x00"
		text = strings.ReplaceAll(text, p.id, token)
		labels = append(labels, token, p.label)
	}
	for i := 0; i < len(labels); i += 2 {
		text = strings.ReplaceAll(text, labels[i], labels[i+1])
	}
	return text
}

func publicReviewHeader(line string) string {
	const prefix = "=== REVIEW FROM "
	rest := strings.TrimPrefix(line, prefix)
	roleAt := strings.Index(rest, " [role:")
	if roleAt < 0 {
		return line
	}
	identity := rest[:roleAt]
	role := rest[roleAt:]
	fields := strings.Fields(identity)
	if len(fields) == 0 {
		return line
	}
	reviewer := strings.Trim(fields[0], "()")
	lowerIdentity := strings.ToLower(identity)
	if strings.Contains(lowerIdentity, "retired-model") {
		return prefix + "retired-model" + role
	}
	switch strings.ToLower(reviewer) {
	case "codex":
		// Sol and Codex share this adapter, so disambiguate by identity
		// before defaulting to Sol. The label is the adapter word itself, so
		// only a bare "codex" identity or the Codex model id means Codex.
		if lowerIdentity == CodexLabel || strings.Contains(lowerIdentity, CodexModel) || strings.Contains(lowerIdentity, "astra") {
			reviewer = CodexLabel
		} else {
			reviewer = SolLabel // read-compat: display of sessions recorded before Sol's removal
		}
	case "claude":
		// Same collision as codex: a bare "claude" or the current id is the
		// Claude reviewer, anything else is a retired Claude-runtime model.
		if lowerIdentity == ClaudeLabel || strings.Contains(lowerIdentity, ClaudeModel) {
			reviewer = ClaudeLabel
		} else {
			reviewer = "retired-model"
		}
	case "opencode":
		if strings.Contains(lowerIdentity, K3Label) {
			reviewer = K3Label
		} else {
			reviewer = "retired-model"
		}
	case GPT56SolModel: // read-compat: display of sessions recorded before Sol's removal
		reviewer = SolLabel
	case ClaudeModel:
		reviewer = ClaudeLabel
	case KimiModel:
		reviewer = K3Label
	case GrokLabel, GrokModel:
		reviewer = GrokLabel
	}
	return prefix + reviewer + role
}

// KimiAPIKeyFrom returns the Moonshot AI API key for K3 runs.
// MOONSHOT_API_KEY is the canonical OpenCode variable; KIMI_API remains a
// backward-compatible alias. Rival checks the process environment first, then
// walks up from workdir looking for either entry in a project .env. The key is
// injected per run into OpenCode's built-in moonshotai provider through
// OPENCODE_CONFIG_CONTENT and is never written to on-disk OpenCode config.
func KimiAPIKeyFrom(workdir string) string {
	for _, name := range []string{"MOONSHOT_API_KEY", "KIMI_API"} {
		if key := strings.TrimSpace(os.Getenv(name)); key != "" {
			return key
		}
	}
	if workdir == "" {
		return ""
	}
	dir, err := filepath.Abs(workdir)
	if err != nil {
		return ""
	}
	home, _ := os.UserHomeDir()
	for i := 0; i < 8; i++ {
		if vars, err := godotenv.Read(filepath.Join(dir, ".env")); err == nil {
			for _, name := range []string{"MOONSHOT_API_KEY", "KIMI_API"} {
				if key := strings.TrimSpace(vars[name]); key != "" {
					return key
				}
			}
		}
		parent := filepath.Dir(dir)
		if dir == home || parent == dir {
			break
		}
		dir = parent
	}
	return ""
}

// SystemPrompt is prepended as a system instruction to all CLI invocations.
const SystemPrompt = `Answer the user's question directly. Do not offer follow-up options, menus, walkthroughs, or ask if they want more. No filler, no sign-offs. Just deliver the answer and stop.`

// WorkdirPreamble tells the CLI which project directory it's operating in.
const WorkdirPreamble = `You are working in project directory: {WORKDIR}
Use your tools to read files, run git commands, and explore the codebase as needed.
`

// BuildWorkdirPreamble returns the workdir preamble with the absolute path injected.
func BuildWorkdirPreamble(workdir string) string {
	abs, _ := filepath.Abs(workdir)
	return strings.ReplaceAll(WorkdirPreamble, "{WORKDIR}", abs)
}

// DiffReviewPreamble is prepended to the reviewer prompt when git auto-detects changed files.
// {FILES} is replaced with the newline-separated file list at runtime.
const DiffReviewPreamble = `The following files have uncommitted changes (or were changed in the last commit). Focus your review on these files, but read other project files as needed for context.

Changed files:
` + "```" + `
{FILES}
` + "```" + `
{DIFFSTAT}
`

// PlanReviewPrompt is the plan/spec review template used by `rival command plan`.
// It targets a single planning/spec markdown document (NOT source code) and asks
// codex to rate it and surface bugs + gaps. {FILE} is replaced with the absolute
// path at the call site. The model must emit ONE JSON object matching the contract
// below so the output can be parsed structurally (see review.ParsePlanOutput).
const PlanReviewPrompt = `You review an engineering PLAN / SPEC document (not source code). Find the real problems that would make this plan fail, mislead an implementer, or ship the wrong thing. Do not report wording nitpicks.

Plan document to review: {FILE}

Read the file in full (use your tools). Judge it as an implementation blueprint. Look for:

1. **Bugs / logic flaws** — steps that are wrong, contradictory, out of order, or that would break when implemented as written.
2. **Gaps** — missing steps, unhandled edge cases, undefined error/failure behavior, absent rollback/migration/auth/validation, things the plan silently assumes.
3. **Ambiguity** — instructions vague enough that two engineers would build different things; unstated assumptions; undefined terms.
4. **Scope / feasibility** — unrealistic claims, hidden dependencies, under-estimated work, or parts that conflict with how the existing system actually works.

When the plan makes claims about existing code (files, functions, schemas, config, commands), open the repo and check them. A claim the code contradicts is a bug finding: cite the plan section in ` + "`file`" + ` and put the contradicting code location in ` + "`body`" + `.
5. **Verification gaps** — no way to tell if the plan succeeded; missing tests, acceptance criteria, or rollback checks.

Rules:
- Only report issues you are confident are real. No speculative nitpicks, no style/grammar comments.
- If the plan is genuinely solid, say so in the summary and return few or zero findings. Do not invent problems.
- Rate the plan overall from 1 (unimplementable / dangerously wrong) to 10 (airtight, ready to execute).

Output: respond with EXACTLY ONE JSON object and nothing else (no prose before or after, no markdown fences). Schema:

{
  "summary": "1-3 sentence overall assessment of the plan",
  "rating": 7,
  "findings": [
    {
      "file": "section or heading the issue is in (or the filename)",
      "line": 0,
      "severity": "critical|high|medium|low",
      "category": "bug|gap|ambiguity|scope|verification",
      "title": "one-line description of the issue",
      "body": "what is wrong and why it matters for implementation",
      "suggestion": "concrete fix or what to add",
      "confidence": 8
    }
  ]
}

Severity guidance: critical = plan is wrong/will cause data loss or a broken build if followed; high = significant gap or flaw that blocks correct implementation; medium = real ambiguity or missing detail an implementer will trip on; low = minor gap or clarification. "line" may be 0 when not applicable. Sort findings by severity, highest first.`

// antislopJSONContract is the output contract shared by both antislop prompts.
// It matches PlanReviewPrompt's schema (summary + rating + findings) so
// review.ParsePlanOutput parses antislop output unchanged. The example summary
// string is intentionally byte-identical to PlanReviewPrompt's — the parser
// uses that exact string to skip prompt echoes.
const antislopJSONContract = `Output: respond with EXACTLY ONE JSON object and nothing else (no prose before or after, no markdown fences). Schema:

{
  "summary": "1-3 sentence overall assessment of the plan",
  "rating": 7,
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "critical|high|medium|low",
      "category": "reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni",
      "title": "one-line description of the cut",
      "body": "what is slop or over-engineered, and what it costs",
      "suggestion": "the concrete cut, merge, defer, or replacement",
      "confidence": 8
    }
  ]
}

"rating" is LEANNESS from 1 (mostly slop / heavily over-engineered) to 10 (nothing left to cut). Severity is the impact of the cut: critical = a large unnecessary subsystem or dependency; high = significant dead weight (unneeded abstraction, layer, or compat path); medium = real but contained slop; low = minor cleanup. "line" may be 0 when not applicable. Sort findings by severity, highest first.`

// AntislopCodePrompt is the quality-only code review template used by
// `rival command antislop` in code mode. {SCOPE} is replaced at runtime.
// Derived from Claude Code's built-in /simplify skill (reuse, simplification,
// efficiency, altitude), extended with over-engineering and AI-slop angles.
// Report-only: the reviewer proposes cuts; the caller applies them.
const AntislopCodePrompt = `You do a QUALITY-ONLY code review: hunt slop and over-engineering, not bugs. Do NOT report correctness bugs, security issues, or missing features — other reviews cover those. Skip style and formatting nitpicks entirely.

Review scope: {SCOPE}

Read the code in the review scope (use your tools; read enclosing functions and neighboring files for context). Work through every angle below, in sequence, in this same pass. For every finding name the concrete cut or replacement.

1. **Reuse & DRY** — new code that re-implements something the codebase already has: search shared/utility modules and files adjacent to the change, and name the existing helper to call instead. Duplicated logic across files or functions is a finding even when each copy is individually fine — name the single home for it.

2. **Simplification** — unnecessary complexity: redundant or derivable state, copy-paste with slight variation, deep nesting, dead code left behind. Name the simpler form that does the same job.

3. **Efficiency** — wasted work: redundant computation or repeated I/O, independent operations run sequentially, blocking work added to startup or hot paths, long-lived objects built from closures that keep the whole enclosing scope alive. Name the cheaper alternative.

4. **Altitude** — changes implemented at the wrong depth: special cases layered on shared infrastructure are a sign the fix is not deep enough — prefer generalizing the underlying mechanism over adding special cases.

5. **Backward-compat hoarding** — compat shims, legacy fallbacks, deprecated-but-kept paths, versioned duplicates (doThingV2), re-export layers kept "just in case". Default stance: delete the old path and migrate the callers. Spare compat code ONLY when a named external consumer (published API, on-disk format, wire protocol) depends on it — name that consumer, otherwise recommend the cut.

6. **Library reinvention** — hand-rolled implementations of what a well-established library already does (parsers, retry/backoff, date math, semver, globbing, and the like). Prefer the language stdlib and the project's existing dependencies before proposing a new one. Name the exact replacement package or function.

7. **Slop signatures** — the telltale patterns of generated code:
- Comment slop: comments narrating the obvious, docstrings restating the signature, section banners, comments justifying the change to a reviewer. Keep only comments stating a constraint the code cannot show.
- Silent-fallback slop: unrequested graceful degradation — quiet defaults on missing config, empty catch blocks returning a zero value. Flag as unspecified behavior masking failures; ask "where was this fallback specified?" rather than asserting what the intended behavior is.
- Wrapper/pass-through slop: functions whose body is a single call, interfaces with one implementation, getters over public fields, grab-bag utils/helpers layers.
- Speculative generality: options nobody passes, parameters always called with the same value, generics instantiated at one type, both directions implemented when only one is needed. Verify via call-site search before reporting.

Rules:
- Only report issues you verified against the code. Every finding cites an exact file and line.
- Prefer fewer, stronger findings over many weak ones.
- If the code is already lean, say so in the summary, give a high rating, and return few or zero findings. Do not invent problems.

` + antislopJSONContract

// WholeProject is the review scope used when none is given and git detects no
// changed files.
const WholeProject = "the entire project"

// PromptKind selects which reviewer prompt BuildReviewerPrompt renders: the
// bug hunter or the security lens.
type PromptKind int

const (
	PromptBugHunter PromptKind = iota
	PromptSecurity
)

// SecurityModel describes one model that can run the security review. Both
// entries run through the OpenCode adapter, so the differences between them
// are data rather than code.
type SecurityModel struct {
	// Name is the config value: "k3" or "grok".
	Name string
	// Model is the upstream model id.
	Model string
	// Selector is what `opencode -m` receives. OpenCode splits it at the
	// first slash to choose the provider, so for OpenRouter-hosted models
	// this differs from Model.
	Selector string
	// Provider names the block in the generated OpenCode config.
	Provider string
	// BaseURL is the provider endpoint. Empty means OpenCode's own default,
	// which is what the built-in Moonshot provider uses.
	BaseURL string
	// KeyEnv is the environment variable holding the API key.
	KeyEnv string
	// Label is the public name shown in output. It must not collide with any
	// other model's label or concrete id.
	Label string
	// Variant is the reasoning level passed to the provider.
	Variant string
}

// SecurityReviewerK3 and SecurityReviewerGrok are the accepted values of the
// security.reviewer config key.
const (
	SecurityReviewerK3   = "k3"
	SecurityReviewerGrok = "grok"
)

// GrokOpenRouterModel is Grok 4.6 served through OpenRouter. It is a
// different runtime from GrokModel, which reaches xAI through the grok CLI.
const (
	GrokOpenRouterModel    = "x-ai/grok-4.6"
	GrokOpenRouterSelector = "openrouter/x-ai/grok-4.6"
	// GrokOpenRouterLabel deliberately differs from GrokLabel. The two Groks
	// are separate models on separate runtimes with separate credentials, and
	// a shared label would make their sessions and logs indistinguishable.
	GrokOpenRouterLabel = "grok-4.6-openrouter"
	openRouterBaseURL   = "https://openrouter.ai/api/v1"
)

// securityModels is the registry. Adding an entry here is all a new security
// model needs: the adapter reads provider, key, selector, and variant from it.
var securityModels = map[string]SecurityModel{
	SecurityReviewerK3: {
		Name:     SecurityReviewerK3,
		Model:    KimiModel,
		Selector: KimiModel,
		Provider: "moonshotai",
		KeyEnv:   "MOONSHOT_API_KEY",
		Label:    K3Label,
		Variant:  "max",
	},
	SecurityReviewerGrok: {
		Name:     SecurityReviewerGrok,
		Model:    GrokOpenRouterModel,
		Selector: GrokOpenRouterSelector,
		Provider: "openrouter",
		BaseURL:  openRouterBaseURL,
		KeyEnv:   "OPENROUTER_API_KEY",
		Label:    GrokOpenRouterLabel,
		Variant:  "xhigh",
	},
}

// SecurityReviewerNames lists the accepted config values, for error messages.
func SecurityReviewerNames() []string {
	return []string{SecurityReviewerK3, SecurityReviewerGrok}
}

// ResolveSecurityModel returns the model that runs the security review. An
// empty config value means K3.
func ResolveSecurityModel() (SecurityModel, error) {
	name := SecurityReviewerK3
	if userConfig != nil {
		if configured := strings.TrimSpace(userConfig.Security.Reviewer); configured != "" {
			name = strings.ToLower(configured)
		}
	}
	entry, ok := securityModels[name]
	if !ok {
		return SecurityModel{}, fmt.Errorf("invalid security.reviewer %q, must be one of: %s",
			name, strings.Join(SecurityReviewerNames(), ", "))
	}
	return entry, nil
}

// ConfiguredSecurityReviewer returns the raw config value, or "" when unset.
func ConfiguredSecurityReviewer() string {
	if userConfig == nil {
		return ""
	}
	return strings.TrimSpace(userConfig.Security.Reviewer)
}

// OpenCodeEntryFor looks up a registry entry by concrete model id, without
// consulting the security config: with security.reviewer set to grok, a run
// that names K3 must still run K3.
func OpenCodeEntryFor(model string) (SecurityModel, bool) {
	for _, entry := range securityModels {
		if entry.Model == model {
			return entry, true
		}
	}
	return SecurityModel{}, false
}

// SecurityAPIKeyFrom resolves an entry's API key. Precedence matches
// KimiAPIKeyFrom: the process environment first, then the nearest .env found
// walking up from workdir.
func SecurityAPIKeyFrom(entry SecurityModel, workdir string) string {
	if entry.Name == SecurityReviewerK3 {
		// Delegate rather than reimplement: KimiAPIKeyFrom checks the legacy
		// KIMI_API alias in every .env it walks, not only in the process
		// environment, and an existing installation may rely on that.
		return KimiAPIKeyFrom(workdir)
	}
	if key := strings.TrimSpace(os.Getenv(entry.KeyEnv)); key != "" {
		return key
	}
	if workdir == "" {
		return ""
	}
	dir, err := filepath.Abs(workdir)
	if err != nil {
		return ""
	}
	home, _ := os.UserHomeDir()
	for i := 0; i < 8; i++ {
		if vars, err := godotenv.Read(filepath.Join(dir, ".env")); err == nil {
			if key := strings.TrimSpace(vars[entry.KeyEnv]); key != "" {
				return key
			}
		}
		parent := filepath.Dir(dir)
		if dir == home || parent == dir {
			break
		}
		dir = parent
	}
	return ""
}

// IsValidEffort checks if the given effort level is in the allowlist.
func IsValidEffort(e string) bool {
	for _, v := range ValidEfforts {
		if v == e {
			return true
		}
	}
	return false
}

// SessionDirPath returns the absolute path to ~/.rival/sessions.
func SessionDirPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(".", SessionDir)
	}
	return filepath.Join(home, SessionDir)
}

// QueueDirPath returns the absolute path to ~/.rival/queue.
func QueueDirPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(".", QueueDir)
	}
	return filepath.Join(home, QueueDir)
}

// Claude auth modes for native runs (RIVAL_CLAUDE_AUTH).
const (
	ClaudeAuthSubscription = "subscription" // CLI's own /login (Pro/Max) — the default
	ClaudeAuthAPI          = "api"          // explicit ANTHROPIC_API_KEY billing
)

// ClaudeAuth returns the auth mode for native claude/claude runs.
// Default is subscription: the claude CLI is already authed via /login, and an
// inherited ANTHROPIC_API_KEY must never silently switch billing to API
// credits. API billing is opt-in via RIVAL_CLAUDE_AUTH=api and then requires
// ANTHROPIC_API_KEY to be set. Any other value is a hard error — auth must be
// explicit, never guessed.
func ClaudeAuth() (string, error) {
	switch v := os.Getenv("RIVAL_CLAUDE_AUTH"); v {
	case "", ClaudeAuthSubscription, "sub":
		return ClaudeAuthSubscription, nil
	case ClaudeAuthAPI:
		if os.Getenv("ANTHROPIC_API_KEY") == "" {
			return "", fmt.Errorf("RIVAL_CLAUDE_AUTH=api but ANTHROPIC_API_KEY is empty — set the key or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login")
		}
		return ClaudeAuthAPI, nil
	default:
		return "", fmt.Errorf("invalid RIVAL_CLAUDE_AUTH=%q — use %q (default) or %q", v, ClaudeAuthSubscription, ClaudeAuthAPI)
	}
}

// MaxConcurrent returns how many reviews may run at once (RIVAL_MAX_CONCURRENT, default 2).
func MaxConcurrent() int {
	if v := os.Getenv("RIVAL_MAX_CONCURRENT"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return n
		}
	}
	return DefaultMaxConcurrent
}

// QueueTimeout returns the max time to wait for a queue slot (RIVAL_QUEUE_TIMEOUT, default 30m).
func QueueTimeout() time.Duration {
	if v := os.Getenv("RIVAL_QUEUE_TIMEOUT"); v != "" {
		if d, err := time.ParseDuration(v); err == nil && d > 0 {
			return d
		}
	}
	return DefaultQueueTimeout
}

// MaxRunWait returns a safe upper bound on how long a detached run can legitimately
// take end-to-end: the full queue wait plus 2× RunTimeout (every current run
// holds a 1× budget; the second is headroom), plus a small margin for process startup,
// stdout flush, and reaper cycles. `rival wait` uses this as its default timeout
// so it never gives up on a run that is still within its configured limits.
// When RunTimeout is disabled (0), only the queue wait + margin is bounded.
func MaxRunWait() time.Duration {
	margin := 5 * time.Minute
	return QueueTimeout() + 2*RunTimeout() + margin
}

// RunTimeout returns the max wall-clock a single provider run may take once it
// holds a queue slot (RIVAL_RUN_TIMEOUT, default 30m). This is the hard
// guarantee that a detached rival always terminates even if the provider CLI
// hangs. The clock starts after slot promotion, so queue wait does not eat it.
// Set RIVAL_RUN_TIMEOUT=0 to disable (no timeout — returns 0); an unset or
// unparseable value falls back to the default.
func RunTimeout() time.Duration {
	v := os.Getenv("RIVAL_RUN_TIMEOUT")
	if v == "" {
		return DefaultRunTimeout
	}
	d, err := time.ParseDuration(v)
	if err != nil {
		return DefaultRunTimeout
	}
	if d < 0 {
		return DefaultRunTimeout
	}
	return d // d == 0 → caller treats as "no timeout"
}

// WithRunTimeout derives a context bounded by mult×RunTimeout(). mult scales the
// budget for multi-phase pipelines; every current caller passes 1.
// When RunTimeout() is 0 (disabled) it returns ctx with a no-op cancel.
func WithRunTimeout(ctx context.Context, mult int) (context.Context, context.CancelFunc) {
	d := RunTimeout()
	if d <= 0 || mult <= 0 {
		return ctx, func() {}
	}
	return context.WithTimeout(ctx, time.Duration(mult)*d)
}

// QueueDisabled reports whether queueing is bypassed via RIVAL_NO_QUEUE.
func QueueDisabled() bool {
	v := os.Getenv("RIVAL_NO_QUEUE")
	return v != "" && v != "0" && !strings.EqualFold(v, "false")
}

// ClaudeConfig holds claude-specific settings.
type ClaudeConfig struct {
	Subscription string `yaml:"subscription"` // "team" or "personal"
}

// UserConfig holds optional user configuration from ~/.rival/config.yaml.
// SecurityConfig selects which model runs the security review.
type SecurityConfig struct {
	Reviewer string `yaml:"reviewer"`
}

type UserConfig struct {
	Claude   ClaudeConfig      `yaml:"claude"`
	Security SecurityConfig    `yaml:"security"`
	Efforts  map[string]string `yaml:"efforts"`
	Roles    map[string]string `yaml:"roles"`
}

var userConfig *UserConfig
var userConfigErr error

// LoadUserConfig reads ~/.rival/config.yaml if it exists.
func LoadUserConfig() {
	userConfig = nil
	userConfigErr = nil
	home, err := os.UserHomeDir()
	if err != nil {
		return
	}
	path := filepath.Join(home, ".rival", "config.yaml")
	data, err := os.ReadFile(path)
	if err != nil {
		if !errors.Is(err, os.ErrNotExist) {
			userConfigErr = fmt.Errorf("read %s: %w", path, err)
		}
		return
	}
	var cfg UserConfig
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		userConfigErr = fmt.Errorf("parse %s: %w", path, err)
		return
	}
	// Sol was removed on 2026-09-26. An old efforts.sol entry configures
	// nothing that can run, so drop it instead of failing every command.
	delete(cfg.Efforts, SolLabel)
	for label, raw := range cfg.Efforts {
		effort := strings.ToLower(strings.TrimSpace(raw))
		if !knownEffortModel(label) {
			userConfigErr = fmt.Errorf("invalid effort model %q in %s; use one of: codex, kimi-k3, claude, grok", label, path)
			return
		}
		if !validConfiguredModelEffort(label, effort) {
			allowed := "low, medium, high, xhigh, ultra"
			if label == "kimi-k3" {
				allowed = "max"
			}
			userConfigErr = fmt.Errorf("invalid effort %q for %s in %s; use one of: %s", raw, label, path, allowed)
			return
		}
		cfg.Efforts[label] = effort
	}
	// Validate the security reviewer here, not only where it is resolved, so a
	// typo fails every command instead of waiting for a security run.
	if configured := strings.TrimSpace(cfg.Security.Reviewer); configured != "" {
		name := strings.ToLower(configured)
		if _, ok := securityModels[name]; !ok {
			userConfigErr = fmt.Errorf("invalid security.reviewer %q in %s; use one of: %s",
				configured, path, strings.Join(SecurityReviewerNames(), ", "))
			return
		}
		cfg.Security.Reviewer = name
	}
	userConfig = &cfg
}

// UserConfigError reports an invalid ~/.rival/config.yaml. Commands fail
// before doing any queue, session, or provider work rather than silently
// ignoring a typo in a requested model default.
func UserConfigError() error {
	return userConfigErr
}

// RolePromptOverride returns the user-configured prompt for a role, if any.
func RolePromptOverride(role string) (string, bool) {
	if userConfig == nil {
		return "", false
	}
	v, ok := userConfig.Roles[role]
	return v, ok
}

// DefaultEffortForModel returns the configured default for a concrete model id
// or public model label. Invalid user configuration is reported by
// UserConfigError before command side effects begin.
//
// Kimi K3 is thinking-only and supports exactly max. Other current models
// accept Rival's low/medium/high/xhigh/ultra ladder.
func DefaultEffortForModel(model string) string {
	label := ModelLabel(model)
	builtin := builtinModelEffort(label)
	if userConfig == nil {
		return builtin
	}
	effort, ok := userConfig.Efforts[label]
	if !ok {
		return builtin
	}
	return effort
}

// titleLabel upper-cases a label's first letter for banner display. Labels
// are lowercase everywhere else, and the codex banner has always read
// "Sol runtime".
func titleLabel(label string) string {
	if label == "" {
		return label
	}
	return strings.ToUpper(label[:1]) + label[1:]
}

// pinnedModelEffort reports models whose effort is a property of the model
// rather than of the surface invoking it. K3's provider exposes exactly one
// level; Codex defaults to xhigh for correctness and plan reviews; Claude
// runs at medium.
func pinnedModelEffort(label string) (string, bool) {
	switch label {
	case "kimi-k3":
		return "max", true
	case CodexLabel:
		return "xhigh", true
	case ClaudeLabel:
		// Claude runs Opus 5.5 at medium on every surface.
		return "medium", true
	default:
		return "", false
	}
}

func builtinModelEffort(label string) string {
	switch label {
	case CodexLabel:
		// Codex is the deep-reasoning model and is pinned to xhigh.
		return "xhigh"
	case "kimi-k3":
		return "max"
	case ClaudeLabel:
		return "medium"
	case GrokLabel:
		// grok-4.6's menu is low/medium/high, and high is its own default.
		return "high"
	default:
		return DefaultReviewEffort
	}
}

func knownEffortModel(label string) bool {
	switch label {
	case CodexLabel, "kimi-k3", ClaudeLabel, GrokLabel:
		return true
	default:
		return false
	}
}

func validConfiguredModelEffort(label, effort string) bool {
	if label == "kimi-k3" {
		return effort == "max"
	}
	return IsValidEffort(effort)
}

// ResolveEffort applies the documented precedence for one concrete model:
// explicit invocation override, then ~/.rival/config.yaml, then the supplied
// surface-specific fallback. Kimi K3 remains pinned to max because that
// provider exposes no other reasoning level.
func ResolveEffort(model, override, fallback string) (string, error) {
	return resolveEffort(model, override, fallback, true)
}

// ResolveAntislopEffort uses the task's cheaper default instead of Codex's
// correctness-review pin. Explicit overrides and configured efforts still win.
func ResolveAntislopEffort(model, override string) (string, error) {
	return resolveEffort(model, override, DefaultAntislopEffort, false)
}

func resolveEffort(model, override, fallback string, pinCodex bool) (string, error) {
	label := ModelLabel(model)
	override = strings.ToLower(strings.TrimSpace(override))
	if override != "" {
		if label == "kimi-k3" {
			return "max", nil
		}
		if !IsValidEffort(override) {
			return "", fmt.Errorf("invalid effort %q for %s", override, label)
		}
		return override, nil
	}
	if userConfig != nil {
		if effort, ok := userConfig.Efforts[label]; ok {
			return effort, nil
		}
	}
	fallback = strings.ToLower(strings.TrimSpace(fallback))
	// A model that pins its own effort outranks a surface-specific fallback.
	// Without this, every caller that passes a non-empty fallback (the
	// review and plan paths both pass one) silently overrides the pin,
	// which is how Codex ran at high instead of xhigh.
	if pinned, ok := pinnedModelEffort(label); ok && (label != CodexLabel || pinCodex) {
		return pinned, nil
	}
	if fallback == "" {
		fallback = builtinModelEffort(label)
	}
	if label == "kimi-k3" {
		return "max", nil
	}
	if !IsValidEffort(fallback) {
		return "", fmt.Errorf("invalid fallback effort %q for %s", fallback, label)
	}
	return fallback, nil
}

// ClaudeSubscription returns the configured subscription type ("team", "personal", or "").
func ClaudeSubscription() string {
	if userConfig == nil {
		return ""
	}
	return userConfig.Claude.Subscription
}

func init() {
	LoadUserConfig()
}
