package session

import (
	"github.com/1905/rival/internal/procinfo"
	"github.com/rs/zerolog/log"
)

// ReapOrphans finds sessions stuck in "running" or "queued" whose process is
// dead, and marks them failed. A session is only an orphan when BOTH its
// tracked process (the provider child, once the subprocess starts) AND its
// owning rival process are dead: between the provider exiting and the owner
// writing the final status, the session file still says "running" with a dead
// provider PID, and a concurrent rival invocation's reap in that window would
// stomp a successful run to failed. A live owner always finalizes its own
// sessions. Sessions from older releases have no owner recorded (OwnerPID 0)
// and keep the provider-only check.
func ReapOrphans() {
	sessions := LoadAllSummaries()
	for _, s := range sessions {
		if s.Status != "running" && s.Status != "queued" {
			continue
		}
		if s.OwnerPID != 0 && procinfo.Alive(s.OwnerPID, s.OwnerPIDStart) {
			continue
		}
		if !procinfo.Alive(s.PID, s.PIDStart) {
			msg := "orphaned (process dead)"
			if s.Status == "queued" {
				msg = "orphaned while queued (process dead)"
			}
			log.Info().Str("session", s.ID).Int("pid", s.PID).Str("status", s.Status).Msg("reaping orphaned session")
			// Reload the complete record only for an orphan we will mutate, so
			// saving the failure preserves its full prompt and all legacy data.
			full, err := Load(s.ID)
			if err != nil || (full.Status != "running" && full.Status != "queued") {
				continue
			}
			if full.OwnerPID != 0 && procinfo.Alive(full.OwnerPID, full.OwnerPIDStart) {
				continue
			}
			if procinfo.Alive(full.PID, full.PIDStart) {
				continue
			}
			_ = full.Fail(1, msg)
		}
	}
}
