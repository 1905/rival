package executor

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync"

	"github.com/1F47E/rival/internal/gitscope"
	"github.com/1F47E/rival/internal/procinfo"
	"github.com/1F47E/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// blockedEnvPrefixes are env var prefixes that should not leak from .env to child CLIs.
// OPENCODE_ is blocked because a reviewed repo's .env could otherwise set
// OPENCODE_PERMISSION (or OPENCODE_CONFIG*) to defeat the read-only reviewer
// sandbox — the reviewer must never take permission/config from the code it reviews.
// Moonshot credentials are blocked because godotenv may load them from a
// reviewed repo. The key reaches OpenCode through OPENCODE_CONFIG_CONTENT
// instead, so no child needs the raw source variable.
// GROK_ and XAI_ are blocked for the same reason: a reviewed repo's .env
// (loaded by godotenv in main.go) must not be able to reconfigure the
// authenticated grok runtime — proxy/base URLs, GROK_HOME, auth helpers — and
// XAI_API_KEY fallback auth is deliberately unsupported, since grok runs on the
// user's own `grok login` credentials.
var blockedEnvPrefixes = []string{
	"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
	"http_proxy", "https_proxy", "all_proxy", "no_proxy",
	"NODE_OPTIONS", "LD_PRELOAD", "DYLD_",
	"OPENCODE_",
	"KIMI_",
	"MOONSHOT_",
	"GROK_",
	"XAI_",
}

// safeEnv returns os.Environ() filtered to block dangerous overrides
// that could be injected via a repo-local .env file.
func safeEnv() []string {
	var result []string
	for _, kv := range gitscope.RepositoryEnv(os.Environ()) {
		blocked := false
		for _, prefix := range blockedEnvPrefixes {
			if strings.HasPrefix(kv, prefix) {
				blocked = true
				break
			}
		}
		if !blocked {
			result = append(result, kv)
		}
	}
	return result
}

// Result holds subprocess execution results.
type Result struct {
	ExitCode    int
	OutputBytes int64
	OutputLines int
}

// syncWriter wraps an io.Writer with a mutex for concurrent use.
type syncWriter struct {
	mu sync.Mutex
	w  io.Writer
}

func (sw *syncWriter) Write(p []byte) (int, error) {
	sw.mu.Lock()
	defer sw.mu.Unlock()
	return sw.w.Write(p)
}

// dropMatches reports whether kv (a KEY=VALUE entry) is named by dropEnv. An
// entry ending in "_" is a prefix match ("AWS_" drops every AWS_* var — exact
// name lists rot as providers add credential vars); any other entry matches
// that exact variable name.
func dropMatches(kv string, dropEnv []string) bool {
	for _, name := range dropEnv {
		if strings.HasSuffix(name, "_") {
			if strings.HasPrefix(kv, name) {
				return true
			}
		} else if strings.HasPrefix(kv, name+"=") {
			return true
		}
	}
	return false
}

// RunSubprocess executes a command, pipes prompt to stdin, tees stdout to log + optional mirror.
// dropEnv names vars to remove from the inherited environment before `env` is
// appended — used e.g. to keep ANTHROPIC_API_KEY away from a subscription-authed
// claude CLI, which would silently prefer the key. An entry ending in "_"
// drops every variable with that prefix (see dropMatches).
func RunSubprocess(ctx context.Context, sess *session.Session, binary string, args []string, env []string, prompt string, mirror io.Writer, dropEnv ...string) (*Result, error) {
	cmd := exec.CommandContext(ctx, binary, args...)
	base := safeEnv()
	if len(dropEnv) > 0 {
		kept := base[:0]
		for _, kv := range base {
			if !dropMatches(kv, dropEnv) {
				kept = append(kept, kv)
			}
		}
		base = kept
	}
	cmd.Env = append(base, env...)
	cmd.Dir = sess.WorkDir

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("stdin pipe: %w", err)
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("stdout pipe: %w", err)
	}

	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, fmt.Errorf("stderr pipe: %w", err)
	}

	logFile, err := sess.OpenLog()
	if err != nil {
		return nil, fmt.Errorf("open log: %w", err)
	}
	defer func() {
		if closeErr := logFile.Close(); closeErr != nil {
			log.Warn().Err(closeErr).Str("session", sess.ID).Msg("failed to close log file")
		}
	}()

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("start %s: %w", binary, err)
	}

	// Record the child PID + its start time (the latter guards the PID against
	// reuse: the queue's liveness check holds a slot while a SIGKILL-orphaned
	// provider child still runs, and must not be fooled by a recycled PID).
	sess.PID = cmd.Process.Pid
	sess.PIDStart, _ = procinfo.StartNanos(cmd.Process.Pid)
	if saveErr := sess.Save(); saveErr != nil {
		log.Warn().Err(saveErr).Str("session", sess.ID).Msg("failed to save session with PID")
	}

	// Thread-safe log writer for concurrent stdout/stderr goroutines.
	safeLog := &syncWriter{w: logFile}

	var outputBytes int64
	var outputLines int
	var stdinErr, scannerErr, stderrErr error
	var wg sync.WaitGroup

	// Write prompt to stdin, then close.
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer func() { _ = stdin.Close() }()
		if _, writeErr := io.WriteString(stdin, prompt); writeErr != nil {
			stdinErr = writeErr
			log.Error().Err(writeErr).Str("session", sess.ID).Msg("failed to write prompt to stdin")
		}
	}()

	// Tee stdout to log file + counter + optional mirror.
	wg.Add(1)
	go func() {
		defer wg.Done()
		var writers []io.Writer
		writers = append(writers, safeLog)
		if mirror != nil {
			writers = append(writers, mirror)
		}
		mw := io.MultiWriter(writers...)
		reader := bufio.NewReader(stdout)
		for {
			line, err := reader.ReadBytes('\n')
			if len(line) > 0 {
				outputLines++
				n, writeErr := mw.Write(line)
				if writeErr != nil {
					log.Error().Err(writeErr).Str("session", sess.ID).Msg("failed to write output line")
				}
				outputBytes += int64(n)
			}
			if err != nil {
				if err != io.EOF {
					scannerErr = err
					log.Warn().Err(err).Str("session", sess.ID).Msg("stdout read error — output may be truncated")
				}
				break
			}
		}
	}()

	// Stderr goes to log file only.
	wg.Add(1)
	go func() {
		defer wg.Done()
		if _, copyErr := io.Copy(safeLog, stderr); copyErr != nil {
			stderrErr = copyErr
			log.Error().Err(copyErr).Str("session", sess.ID).Msg("failed to copy stderr to log")
		}
	}()

	wg.Wait()
	err = cmd.Wait()

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return nil, fmt.Errorf("subprocess %s: %w", binary, err)
		}
	}

	// Only report stdinErr if child exited 0 — broken pipe is expected when child dies early.
	if stdinErr != nil && exitCode == 0 {
		return nil, fmt.Errorf("write prompt to stdin: %w", stdinErr)
	}
	if scannerErr != nil {
		log.Warn().Err(scannerErr).Str("session", sess.ID).Msg("stdout was truncated")
	}
	if stderrErr != nil {
		log.Warn().Err(stderrErr).Str("session", sess.ID).Msg("stderr capture failed")
	}

	return &Result{
		ExitCode:    exitCode,
		OutputBytes: outputBytes,
		OutputLines: outputLines,
	}, nil
}
