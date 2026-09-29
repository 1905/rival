package cmd

import (
	"context"
	"fmt"
	"os"

	"github.com/1905/rival/internal/mergerequest"
	"github.com/rs/zerolog/log"
)

// prepareMR resolves a GitLab MR into a pinned checkout. It is a package var
// so tests can fake it without a network or a git remote.
var prepareMR = mergerequest.Prepare

// reviewTarget is where a single-model review runs and what it reviews.
type reviewTarget struct {
	scope   string // what the prompt reviews; for an MR, the snapshot scope with the patch
	display string // what the session records and the output shows; for an MR, the URL
	workdir string // where the reviewer runs; for an MR, the temporary checkout
	close   func() // removes the MR checkout; a no-op otherwise
}

// prepareReviewTarget pins a GitLab MR scope to a snapshot checkout and prints
// its identity line first on stdout. Any other scope passes through unchanged.
// Call close after the run, on every path.
func prepareReviewTarget(ctx context.Context, scope, workdir string) (reviewTarget, error) {
	plain := reviewTarget{scope: scope, display: scope, workdir: workdir, close: func() {}}
	if !mergerequest.Contains(scope) {
		return plain, nil
	}
	snapshot, err := prepareMR(ctx, scope, workdir)
	if err != nil {
		return reviewTarget{}, err
	}
	if snapshot == nil {
		return plain, nil
	}
	_, _ = fmt.Fprintln(os.Stdout, snapshot.Identity+"\n")
	return reviewTarget{
		scope:   snapshot.Scope,
		display: scope,
		workdir: snapshot.Workdir,
		close: func() {
			if err := snapshot.Close(); err != nil {
				log.Warn().Err(err).Str("path", snapshot.Workdir).Msg("remove MR checkout")
			}
		},
	}, nil
}

// Raw prompts cannot resolve remote identity inside a network-isolated model,
// so reject MR URLs before any reviewer starts and point at the review path,
// which pins the checkout first.
func rejectUnresolvedMR(prompt string) error {
	if mergerequest.Contains(prompt) {
		return fmt.Errorf("GitLab MR URLs need a pinned review: use rival command codex review <MR-URL> (or /rival-codex review <MR-URL>) from a repository with the MR's remote; no reviewer was started")
	}
	return nil
}
