package cmd

import (
	"fmt"
	"os"
	"path/filepath"
)

// resolveWorkdir turns the --workdir flag into an absolute, cleaned path that
// must name an existing directory. It runs once at each command entry, before
// preflight and session creation: the executors set cmd.Dir to the workdir
// and also pass it to the provider (codex -C, opencode --dir, grok --cwd), so
// a relative path would be applied twice ("v3/" -> v3/v3/), and sessions would
// record "." instead of the project path.
func resolveWorkdir(raw string) (string, error) {
	abs, err := filepath.Abs(raw)
	if err != nil {
		return "", fmt.Errorf("resolve workdir %q: %w", raw, err)
	}
	info, err := os.Stat(abs)
	if err != nil {
		if os.IsNotExist(err) {
			return "", fmt.Errorf("workdir not found: %s", abs)
		}
		return "", fmt.Errorf("cannot read workdir %s: %w", abs, err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("workdir is not a directory: %s", abs)
	}
	return abs, nil
}

// resolveWorkdirOrExit is resolveWorkdir for a command action: a bad workdir
// is printed to stdout, where the calling skill captures it, and ends the
// command with exit code 1 — the same contract as an invalid-argument error.
func resolveWorkdirOrExit(raw string) (string, error) {
	abs, err := resolveWorkdir(raw)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return "", &ExitCodeError{Code: 1, Err: err}
	}
	return abs, nil
}
