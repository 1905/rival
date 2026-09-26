package cmd

import (
	"context"
	"io"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/executor"
	"github.com/1F47E/rival/internal/parser"
	"github.com/1F47E/rival/internal/session"
)

// The executor signatures are not uniform: grok takes a review flag, K3 takes
// no effort, and the others take effort but no flag. Each adapter below
// absorbs that difference so both workflows can call one shape.

func solSpec() modelSpec {
	return modelSpec{
		commandName: config.SolLabel,
		cli:         "codex",
		model:       config.GPT56SolModel,
		usage:       solUsage,
		parse:       parser.ParseGPT56SolArgs,
		preflight:   func(string) error { return executor.CodexPreflight() },
		run: func(ctx context.Context, sess *session.Session, prompt, effort, workdir string, _ bool, out io.Writer) (*executor.Result, error) {
			return executor.RunCodexModel(ctx, sess, prompt, effort, workdir, config.GPT56SolModel, out)
		},
	}
}

func codexSpec() modelSpec {
	return modelSpec{
		commandName: config.CodexLabel,
		cli:         "codex",
		model:       config.CodexModel,
		usage:       codexUsage,
		parse:       parser.ParseCodexArgs,
		preflight:   func(string) error { return executor.CodexPreflightFor(config.CodexModel) },
		run: func(ctx context.Context, sess *session.Session, prompt, effort, workdir string, _ bool, out io.Writer) (*executor.Result, error) {
			return executor.RunCodexModel(ctx, sess, prompt, effort, workdir, config.CodexModel, out)
		},
	}
}

func claudeSpec() modelSpec {
	return modelSpec{
		commandName: config.ClaudeLabel,
		cli:         "claude",
		model:       config.ClaudeModel,
		usage:       claudeUsage,
		parse:       parser.ParseClaudeArgs,
		preflight:   func(string) error { return executor.ClaudePreflight() },
		run: func(ctx context.Context, sess *session.Session, prompt, effort, workdir string, _ bool, out io.Writer) (*executor.Result, error) {
			return executor.RunClaude(ctx, sess, prompt, effort, workdir, out)
		},
	}
}

func k3Spec() modelSpec {
	return modelSpec{
		commandName: config.K3CommandName,
		cli:         "opencode",
		model:       config.KimiModel,
		usage:       k3Usage,
		parse:       parser.ParseKimiArgs,
		preflight:   executor.KimiPreflight,
		run: func(ctx context.Context, sess *session.Session, prompt, _, workdir string, _ bool, out io.Writer) (*executor.Result, error) {
			// K3 takes no effort: its provider exposes only max reasoning.
			return executor.RunKimi(ctx, sess, prompt, workdir, out)
		},
	}
}

func grokSpec() modelSpec {
	return modelSpec{
		commandName: config.GrokLabel,
		cli:         config.GrokLabel,
		model:       config.GrokModel,
		usage:       grokUsage,
		parse:       parser.ParseGrokArgs,
		preflight:   func(string) error { return executor.GrokPreflight() },
		run: func(ctx context.Context, sess *session.Session, prompt, effort, workdir string, isReview bool, out io.Writer) (*executor.Result, error) {
			// Grok sandboxes reviews and only reviews.
			return executor.RunGrok(ctx, sess, prompt, effort, workdir, isReview, out)
		},
	}
}
