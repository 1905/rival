package dashboard

import (
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// failSessionForKill marks a session failed after the user kills it.
//
// The list holds summaries, which never carry the full prompt. Session.Fail
// saves the whole in-memory record, so failing a summary directly writes an
// empty prompt over the stored one. This reloads the full record first and
// fails that instead. When the reload fails, it falls back to the in-memory
// record, because a lost prompt is better than a session stuck in "running".
func failSessionForKill(s *session.Session, exitCode int, reason string) {
	target := s
	if full, err := session.Load(s.ID); err == nil && full != nil {
		target = full
	} else if err != nil {
		log.Warn().Err(err).Str("session", s.ID).Msg("could not reload session before kill; failing the in-memory copy")
	}

	if err := target.Fail(exitCode, reason); err != nil {
		log.Warn().Err(err).Str("session", s.ID).Msg("failed to save session failure")
		return
	}

	// Keep the row the user is looking at consistent with what was stored.
	s.Status = target.Status
	s.ExitCode = target.ExitCode
	s.ErrorMsg = target.ErrorMsg
	s.EndTime = target.EndTime
	s.Duration = target.Duration
}
