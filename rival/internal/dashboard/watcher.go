package dashboard

import (
	"context"
	"os"
	"path/filepath"
	"strings"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
	"github.com/1905/rival/internal/sessionview"
	"github.com/fsnotify/fsnotify"
	"github.com/rs/zerolog/log"
)

// SessionEvent is sent when sessions change.
type SessionEvent struct {
	Sessions []*session.Session
}

// LoadProgress reports the initial session scan: Done of Total session files
// read. The dashboard shows it as the startup loader.
type LoadProgress struct {
	Done, Total int
}

// WatchSessions watches the session directory and sends events on changes.
// The goroutine exits when ctx is cancelled.
//
// progress, when not nil, gets LoadProgress messages during the initial scan
// and is closed once that scan is done. Sends never block: a full channel
// drops the update, because the next one (or the first SessionEvent)
// supersedes it.
func WatchSessions(ctx context.Context, events chan<- SessionEvent, progress chan<- LoadProgress) error {
	if progress != nil {
		defer close(progress)
	}
	dir := config.SessionDirPath()
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return err
	}

	if err := watcher.Add(dir); err != nil {
		_ = watcher.Close()
		return err
	}

	// One shared cache serves every reload below. It reparses only the files
	// whose size or mtime changed, instead of re-reading every session JSON
	// (prompts included) on each event.
	cache := sessionview.New(dir)

	// Send initial state.
	var report func(done, total int)
	if progress != nil {
		report = func(done, total int) {
			select {
			case progress <- LoadProgress{Done: done, Total: total}:
			default:
			}
		}
	}
	sessions, _ := cache.LoadWithProgress(report)
	select {
	case events <- SessionEvent{Sessions: sessions}:
	case <-ctx.Done():
		_ = watcher.Close()
		return ctx.Err()
	}

	var lastRevision uint64
	go func() {
		defer func() { _ = watcher.Close() }()
		for {
			select {
			case <-ctx.Done():
				return
			case event, ok := <-watcher.Events:
				if !ok {
					return
				}
				if event.Has(fsnotify.Write) || event.Has(fsnotify.Create) || event.Has(fsnotify.Remove) {
					isJSON := session.IsSessionFile(filepath.Base(event.Name))
					isLog := strings.HasSuffix(event.Name, ".log")
					if isJSON || isLog {
						sessions, revision := cache.Load()
						// Log appends fire constantly during a run. When nothing
						// the dashboard displays changed, skip the redraw.
						if revision == lastRevision && !isJSON {
							continue
						}
						lastRevision = revision
						// A blocked send would stall this goroutine and leak it
						// past cancellation, because the buffered channel fills
						// under log churn.
						select {
						case events <- SessionEvent{Sessions: sessions}:
						case <-ctx.Done():
							return
						}
					}
				}
			case err, ok := <-watcher.Errors:
				if !ok {
					return
				}
				log.Warn().Err(err).Msg("watcher error")
			}
		}
	}()

	return nil
}
