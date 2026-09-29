package cmd

import (
	"errors"
	"fmt"
	"os"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/queue"
	"github.com/1905/rival/internal/session"
	"github.com/1905/rival/internal/telemetry"
	"github.com/1905/rival/internal/update"
	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"
	"github.com/spf13/cobra"
)

// Version is set via ldflags.
var Version = "dev"

const banner = `
         _             __
   _____(_)   ______ _/ /
  / ___/ / | / / __ ` + "`" + `/ /
 / /  / /| |/ / /_/ / /
/_/  /_/ |___/\__,_/_/
`

// ExitCodeError wraps an error with a specific exit code.
type ExitCodeError struct {
	Code int
	Err  error
}

func (e *ExitCodeError) Error() string { return e.Err.Error() }
func (e *ExitCodeError) Unwrap() error { return e.Err }

var rootCmd = &cobra.Command{
	Use:           "rival",
	Short:         "Dispatch prompts and reviews to external AI models",
	SilenceErrors: true,
	SilenceUsage:  true,
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		if err := config.UserConfigError(); err != nil {
			return err
		}
		// Detach before any side effects: the re-exec'd child redoes this hook.
		// GetBool errors (flag absent on non-command subcommands) → false.
		if detach, _ := cmd.Flags().GetBool("detach"); detach {
			detachIfRequested(true)
		}
		// The TUI draws its loader at once and reaps in the background; the
		// watcher picks up every session the reaper fails. Every other
		// command reaps before it runs, as before.
		if cmd == tuiCmd {
			// The TUI owns the terminal: a log line on stderr (the background
			// reaper logs each orphan it fails) draws over the screen.
			log.Logger = zerolog.Nop()
			startReap()
		} else {
			reap()
		}
		startUpdateCheck()
		return nil
	},
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Print(banner)
		fmt.Printf("  %s — multi-model AI reviews from your terminal\n\n", Version)
		cmd.SetOut(os.Stdout)
		_ = cmd.Usage()
	},
}

// reap fails orphaned sessions, then drops dead queue tickets. Sessions
// first: queue ticket liveness reads session state.
func reap() {
	session.ReapOrphans()
	queue.New().ReapDead()
}

// reapDone is closed when a background reap finishes. It is nil when the reap
// ran in the foreground.
var reapDone chan struct{}

// startReap runs reap off the startup path. On a large session dir it costs
// as much as the TUI's own first scan (0.35-0.6 s on 3000 sessions), and the
// terminal stayed blank for all of it.
func startReap() {
	done := make(chan struct{})
	reapDone = done
	go func() {
		defer close(done)
		reap()
	}()
}

// waitForReap lets a background reap finish before the process exits, so
// os.Exit never cuts a session save short and strands its temp file.
func waitForReap() {
	if reapDone != nil {
		<-reapDone
	}
}

// updateCheckDone is closed when the background update check finishes. It is
// nil when no check was started.
var updateCheckDone chan struct{}

// startUpdateCheck runs the release check off the startup path. A stale cache
// makes update.Check perform an HTTP GET, and every command used to pay that
// latency before doing any work.
func startUpdateCheck() {
	done := make(chan struct{})
	updateCheckDone = done
	go func() {
		defer close(done)
		update.Check(Version)
	}()
}

// waitForUpdateCheck gives the background check a bounded moment to print its
// notice after the command finishes. The bound matches the check's own HTTP
// timeout, so a slow network delays nothing beyond what it already budgets.
func waitForUpdateCheck() {
	if updateCheckDone == nil {
		return
	}
	select {
	case <-updateCheckDone:
	case <-time.After(updateCheckWait):
	}
}

const updateCheckWait = 2 * time.Second

func Execute() {
	defer telemetry.RecoverPanic()
	err := rootCmd.Execute()
	// Both exit paths below call os.Exit, so the joins have to happen first.
	waitForReap()
	waitForUpdateCheck()
	if err != nil {
		var exitErr *ExitCodeError
		if errors.As(err, &exitErr) {
			_, _ = fmt.Fprintln(os.Stderr, exitErr.Err)
			os.Exit(exitErr.Code)
		}
		_, _ = fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
