package cmd

import (
	"context"
	"fmt"
	"os"

	"github.com/1F47E/rival/internal/mergerequest"
	"github.com/rs/zerolog/log"
)

func prepareReviewScope(ctx context.Context, scope, workdir string) (string, string, func(), error) {
	snapshot, err := mergerequest.Prepare(ctx, scope, workdir)
	if err != nil {
		return "", "", nil, err
	}
	if snapshot == nil {
		return scope, workdir, func() {}, nil
	}
	_, _ = fmt.Fprintln(os.Stdout, snapshot.Identity+"\n")
	return snapshot.Scope, snapshot.Workdir, func() {
		if err := snapshot.Close(); err != nil {
			log.Warn().Err(err).Str("path", snapshot.Workdir).Msg("remove MR checkout")
		}
	}, nil
}

// Raw prompts cannot resolve remote identity inside a network-isolated model.
// Route them to the entry points that prepare the checkout before dispatch.
func rejectUnresolvedMR(prompt string) error {
	if mergerequest.Contains(prompt) {
		return fmt.Errorf("GitLab MR URLs require a pinned review: use rival review --model astra <MR-URL> --workdir <repository>, or /rival-review <MR-URL>; no reviewer was started")
	}
	return nil
}
