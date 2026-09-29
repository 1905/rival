package cmd

import (
	"strings"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/gitscope"
	"github.com/1905/rival/internal/review"
	"github.com/rs/zerolog/log"
)

// buildDiffPreamble builds the DiffReviewPreamble block (changed-file list +
// diff stats) for workdir. files=="" means git detected no changes; the raw
// files list is returned alongside so callers can record it as the review
// scope without a second gitscope.Resolve fork.
func buildDiffPreamble(workdir string) (preamble, files string) {
	files = gitscope.Resolve(workdir)
	if files == "" {
		return "", ""
	}
	preamble = strings.ReplaceAll(config.DiffReviewPreamble, "{FILES}", files)
	if diffStat := gitscope.DiffStat(workdir); diffStat != "" {
		preamble = strings.ReplaceAll(preamble, "{DIFFSTAT}", "\nDiff stats:\n```\n"+diffStat+"\n```\n")
	} else {
		preamble = strings.ReplaceAll(preamble, "{DIFFSTAT}", "")
	}
	return preamble, files
}

// buildReviewPrompt builds a review prompt with build, which renders the
// prompt for one scope string. With autoScope it asks git for the changed
// files and ignores scope: when there are some, the prompt is
// DiffReviewPreamble + build("the changed files listed above"), target is the
// file list and display is "changed files (git auto-detect)"; when there are
// none it reviews config.WholeProject. Otherwise it reviews scope as given.
// target is what a session records; display is the output's Scope line.
func buildReviewPrompt(build func(scope string) string, scope string, autoScope bool, workdir string) (prompt, target, display string) {
	if autoScope {
		preamble, files := buildDiffPreamble(workdir)
		if files != "" {
			log.Info().Str("files", files).Msg("git scope: auto-detected changed files")
			return preamble + build("the changed files listed above"), files, "changed files (git auto-detect)"
		}
		log.Debug().Msg("git scope: no changes detected, falling back to full project")
		scope = config.WholeProject
	}
	return build(scope), scope, scope
}

// lensPrompt returns the reviewer prompt builder for one lens.
func lensPrompt(kind config.PromptKind) func(string) string {
	return func(scope string) string { return review.BuildReviewerPrompt(scope, kind) }
}

// antislopCodePrompt renders the code-mode antislop prompt for scope.
func antislopCodePrompt(scope string) string {
	return strings.ReplaceAll(config.AntislopCodePrompt, "{SCOPE}", scope)
}
