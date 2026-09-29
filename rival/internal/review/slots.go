package review

import (
	"context"
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/queue"
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// SkippedCLI records a reviewer that was unavailable or failed during a review.
type SkippedCLI struct {
	CLI    string
	Model  string
	Reason string
}

// Label returns the display label for a skipped reviewer.
func (s SkippedCLI) Label() string {
	return config.EngineLabel(s.CLI, s.Model)
}

// WaitForGroupSlot enqueues one ticket covering ticketSessions and blocks
// until a slot is free, then marks runSessions running. Callers today pass the
// same slice for both; the split lets a session hold a ticket for liveness
// before it starts.
//
// Progress goes to stderr with the "rival queue:" prefix, because stdout
// carries the final output that skills present verbatim. On cancel or timeout
// the sessions are failed with a clear reason. Call the returned release via
// defer to free the slot.
func WaitForGroupSlot(ctx context.Context, noQueue bool, ticketSessions, runSessions []*session.Session, workdir, groupID, mode string) (release func(), err error) {
	markRunning := func() error {
		for i, s := range runSessions {
			if err := s.MarkRunning(); err != nil {
				// Roll back any session already flipped to running, so a partial
				// failure never strands a session "running" with no process.
				for _, prev := range runSessions[:i] {
					_ = prev.Fail(1, "aborted: failed to start review batch")
				}
				return fmt.Errorf("mark session running: %w", err)
			}
		}
		return nil
	}

	if noQueue || config.QueueDisabled() {
		return func() {}, markRunning()
	}

	ids := make([]string, len(ticketSessions))
	for i, s := range ticketSessions {
		ids[i] = s.ID
	}

	m := queue.New()
	if _, enqErr := m.Enqueue(groupID, ids, mode, workdir); enqErr != nil {
		log.Warn().Err(enqErr).Str("mode", mode).Msg("queue unavailable — running without queueing")
		return func() {}, markRunning()
	}

	start := time.Now()
	waitErr := m.WaitForSlot(ctx, func(pos, total, running int) {
		_, _ = fmt.Fprintf(os.Stderr, "rival queue: position %d/%d (%d running), waiting %s\n",
			pos, total, running, time.Since(start).Round(time.Second))
		for _, s := range ticketSessions {
			_ = s.SetQueuePosition(pos)
		}
	})
	if waitErr != nil {
		m.Release()
		msg := "cancelled while queued"
		if errors.Is(waitErr, queue.ErrQueueTimeout) {
			msg = fmt.Sprintf("queue timeout after %s — queue may be wedged; inspect with 'rival queue', purge with 'rival queue clear'", m.Timeout)
		}
		for _, s := range ticketSessions {
			_ = s.Fail(1, msg)
		}
		return nil, fmt.Errorf("rival queue: %s", msg)
	}

	if err := markRunning(); err != nil {
		m.Release()
		return nil, err
	}
	if waited := time.Since(start); waited >= time.Second {
		_, _ = fmt.Fprintf(os.Stderr, "rival queue: slot acquired after %s\n", waited.Round(time.Second))
	}
	return m.Release, nil
}

// formatSkipped renders skipped reviewers as "cli: reason" pairs for error messages.
func formatSkipped(skipped []SkippedCLI) string {
	if len(skipped) == 0 {
		return "none"
	}
	parts := make([]string, 0, len(skipped))
	for _, s := range skipped {
		reason := config.PublicRuntimeError(s.CLI, s.Model, s.Reason)
		parts = append(parts, fmt.Sprintf("%s: %s", s.Label(), reason))
	}
	return strings.Join(parts, "; ")
}
