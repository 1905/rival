package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/review"
	"github.com/google/uuid"
	"github.com/spf13/cobra"
)

const planUsage = `Usage:
  /rival-plan path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-codex path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-claude path/to/plan.md — review with Claude
  rival command plan --help — show native command options

Input is a single path to a markdown plan/spec file. The /rival-plan and
/rival-plan-codex skills always use xhigh. Native Codex effort defaults to xhigh
and Claude to medium, unless overridden per model in ~/.rival/config.yaml.
--model accepts codex and claude. An unavailable model is skipped, not fatal.`

var defaultPlanModels = []string{config.CodexLabel}

var commandPlanCmd = &cobra.Command{
	Use:   "plan",
	Short: "Review a plan/spec with Codex and/or Claude",
	RunE:  commandPlanAction,
}

func init() {
	commandPlanCmd.Flags().String("workdir", ".", "working directory")
	commandPlanCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandPlanCmd.Flags().StringSliceP("model", "m", defaultPlanModels, "plan review model(s): codex, claude (comma-separated)")
	commandCmd.AddCommand(commandPlanCmd)
	commandPlanCmd.Flags().String("effort", "", "override reasoning effort for every selected model: "+strings.Join(config.ValidEfforts, ", ")+" (default: each model's own)")
}

// parsePlanModels validates model-facing selectors and maps them to the
// internal adapters used by RunPlanReview. It de-duplicates by concrete model
// while preserving the user's order.
func parsePlanModels(raw []string) ([]string, error) {
	seen := make(map[string]bool)
	var out []string
	for _, value := range raw {
		for _, part := range strings.Split(value, ",") {
			model := strings.ToLower(strings.TrimSpace(part))
			var cli string
			switch model {
			case config.CodexLabel, config.CodexModel:
				cli = "codex"
			case config.ClaudeLabel, config.ClaudeModel:
				cli = "claude"
			case "":
				return nil, fmt.Errorf("model selector cannot be empty")
			default:
				return nil, fmt.Errorf("unknown plan model %q; use one of: codex, claude", part)
			}
			if !seen[cli] {
				seen[cli] = true
				out = append(out, cli)
			}
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("no plan models selected")
	}
	return out, nil
}

func commandPlanAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	workdir, err := resolveWorkdirOrExit(workdir)
	if err != nil {
		return err
	}
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	rawModels, _ := cmd.Flags().GetStringSlice("model")
	effort, _ := cmd.Flags().GetString("effort")
	effortSet := cmd.Flags().Changed("effort")
	if !effortSet {
		// Cobra retains the historical high flag default for help/API
		// compatibility. Keep an omitted invocation empty so each selected
		// model can resolve its own configured or surface-specific default.
		effort = ""
	}

	if effort != "" && !config.IsValidEffort(effort) {
		err := fmt.Errorf("invalid effort %q, must be one of: %v", effort, config.ValidEfforts)
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	clis, err := parsePlanModels(rawModels)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	// If stdin is a terminal, show usage instead of hanging. Guard against a nil
	// stat (stdin closed/invalid) so we don't panic dereferencing it.
	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) != 0 {
		_, _ = fmt.Fprintln(os.Stdout, planUsage)
		return nil
	}

	raw, err := io.ReadAll(os.Stdin)
	if err != nil {
		return fmt.Errorf("read stdin: %w", err)
	}

	rawPath, inputEffort, err := parsePlanInput(string(raw))
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	effort, err = mergePlanEffort(effort, effortSet, inputEffort)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	if rawPath == "" {
		_, _ = fmt.Fprintln(os.Stdout, planUsage)
		return nil
	}

	absPath, err := resolvePlanPath(rawPath, workdir)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	// Cancel the queue wait / child processes on SIGINT/SIGTERM.
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	groupID := uuid.New().String()

	result, err := review.RunPlanReview(ctx, absPath, effort, workdir, groupID, noQueue, clis)
	if err != nil {
		return err
	}

	out := review.FormatPlanResult(result, absPath)
	if _, err := io.WriteString(os.Stdout, out); err != nil {
		return fmt.Errorf("write stdout: %w", err)
	}
	return nil
}

func mergePlanEffort(flagEffort string, flagSet bool, inputEffort string) (string, error) {
	if inputEffort == "" {
		return flagEffort, nil
	}
	if flagSet && inputEffort != flagEffort {
		return "", fmt.Errorf("reasoning effort conflicts: command uses %q but plan arguments request %q", flagEffort, inputEffort)
	}
	return inputEffort, nil
}

// parsePlanInput extracts an optional skill-facing -re/--effort prefix while
// leaving the rest of the input intact as the path (including spaces). Native
// callers can use --effort; slash-command skills pass `-re xhigh plan.md` on
// stdin. A leading `--` escapes a path beginning with a dash.
func parsePlanInput(raw string) (path, effort string, err error) {
	s := strings.TrimSpace(raw)
	if s == "" {
		return "", "", nil
	}
	if strings.HasPrefix(s, "-- ") {
		return strings.TrimSpace(strings.TrimPrefix(s, "--")), "", nil
	}

	option, rest := popPlanToken(s)
	name, inline, hasInline := splitPlanOption(option)
	if name != "-re" && name != "--effort" {
		if strings.HasPrefix(option, "-") {
			return "", "", fmt.Errorf("unknown plan option %q; use -re/--effort or -- before a path beginning with '-'", option)
		}
		return s, "", nil
	}

	if hasInline {
		effort = strings.TrimSpace(inline)
	} else {
		if strings.TrimSpace(rest) == "" {
			return "", "", fmt.Errorf("option %s requires a value", name)
		}
		effort, rest = popPlanToken(rest)
	}
	if effort == "" || strings.HasPrefix(effort, "-") {
		return "", "", fmt.Errorf("option %s requires a value", name)
	}
	if !config.IsValidEffort(effort) {
		return "", "", fmt.Errorf("invalid effort %q, must be one of: %s", effort, strings.Join(config.ValidEfforts, ", "))
	}
	path = strings.TrimSpace(rest)
	if path == "" {
		return "", "", fmt.Errorf("plan path is required after %s %s", name, effort)
	}
	return path, effort, nil
}

func popPlanToken(s string) (token, rest string) {
	s = strings.TrimLeft(s, " \t\r\n")
	if i := strings.IndexAny(s, " \t\r\n"); i >= 0 {
		return s[:i], strings.TrimLeft(s[i:], " \t\r\n")
	}
	return s, ""
}

func splitPlanOption(token string) (name, value string, hasValue bool) {
	if i := strings.Index(token, "="); i >= 0 {
		return token[:i], token[i+1:], true
	}
	return token, "", false
}

// resolvePlanPath turns the raw user-supplied path into a validated absolute path
// to an existing regular file. Relative paths are resolved against workdir. The
// .md extension is preferred but not required (lenient).
func resolvePlanPath(rawPath, workdir string) (string, error) {
	p := strings.TrimSpace(rawPath)
	// Expand a leading ~ to the home directory.
	if p == "~" || strings.HasPrefix(p, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			p = filepath.Join(home, strings.TrimPrefix(p, "~"))
		}
	}
	if !filepath.IsAbs(p) {
		p = filepath.Join(workdir, p)
	}
	// Reject control characters (e.g. a newline in the filename): the path is
	// interpolated into the model prompt, so a control char could inject prompt
	// text. Real plan files never have these in their path.
	if i := strings.IndexFunc(p, func(r rune) bool { return r < 0x20 || r == 0x7f }); i >= 0 {
		return "", fmt.Errorf("plan path contains a control character at position %d — refusing", i)
	}

	abs, err := filepath.Abs(p)
	if err != nil {
		return "", fmt.Errorf("resolve plan path %q: %w", rawPath, err)
	}

	info, err := os.Stat(abs)
	if err != nil {
		if os.IsNotExist(err) {
			return "", fmt.Errorf("plan file not found: %s", abs)
		}
		return "", fmt.Errorf("cannot read plan file %s: %w", abs, err)
	}
	if info.IsDir() {
		return "", fmt.Errorf("plan path is a directory, not a file: %s", abs)
	}
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("plan path is not a regular file: %s", abs)
	}
	// Confirm the file is actually readable now, so an unreadable file fails here
	// with a clear message rather than later inside a model runner with an opaque error.
	f, err := os.Open(abs)
	if err != nil {
		return "", fmt.Errorf("cannot read plan file %s: %w", abs, err)
	}
	_ = f.Close()
	return abs, nil
}
