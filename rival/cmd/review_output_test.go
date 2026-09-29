package cmd

import (
	"context"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/1905/rival/internal/executor"
	"github.com/1905/rival/internal/mergerequest"
	"github.com/1905/rival/internal/session"
)

// Commands removed on 2026-09-26 must not resolve: `rival review`,
// `rival command megareview`, the Sol commands and the web dashboard (`server`).
func TestRemovedReviewCommandsDoNotResolve(t *testing.T) {
	for _, tc := range []struct {
		path []string
		name string
	}{
		{[]string{"review"}, "review"},
		{[]string{"command", "megareview"}, "megareview"},
		// Sol was removed on 2026-09-26; codex is its replacement.
		{[]string{"command", "sol"}, "sol"},
		{[]string{"run", "sol"}, "sol"},
		// The web dashboard was removed on 2026-09-26; Rival.app and
		// `rival tui` replace it.
		{[]string{"server"}, "server"},
	} {
		found, _, _ := rootCmd.Find(tc.path)
		if found != nil && found.Name() == tc.name {
			t.Errorf("%v still resolves to command %q", tc.path, found.Name())
		}
	}
}

// fakeRun records what the provider was handed and writes log as its output.
type fakeRun struct {
	log         string
	exitCode    int
	called      bool
	prompt      string
	workdir     string
	credWorkdir string
	review      bool
	// workdirExisted is whether workdir was still on disk during the run, so
	// a test can tell the MR checkout was closed after, not before.
	workdirExisted bool
}

func (f *fakeRun) spec() modelSpec {
	spec := codexSpec()
	spec.preflight = func(string) error { return nil }
	spec.run = func(_ context.Context, sess *session.Session, prompt, _, workdir, credWorkdir string, review bool, _ io.Writer) (*executor.Result, error) {
		f.called, f.prompt, f.workdir, f.credWorkdir, f.review = true, prompt, workdir, credWorkdir, review
		_, statErr := os.Stat(workdir)
		f.workdirExisted = statErr == nil
		if err := os.WriteFile(sess.LogFile, []byte(f.log), 0o600); err != nil {
			return nil, err
		}
		return &executor.Result{ExitCode: f.exitCode, OutputBytes: int64(len(f.log))}, nil
	}
	return spec
}

// withStdin points os.Stdin at a file holding input for the test's duration.
func withStdin(t *testing.T, input string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "stdin")
	if err := os.WriteFile(path, []byte(input), 0o600); err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	previous := os.Stdin
	os.Stdin = f
	t.Cleanup(func() {
		os.Stdin = previous
		_ = f.Close()
	})
}

// captureStdout runs fn and returns what it wrote to os.Stdout.
func captureStdout(t *testing.T, fn func() error) (string, error) {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	previous := os.Stdout
	os.Stdout = w
	done := make(chan string)
	go func() {
		data, _ := io.ReadAll(r)
		done <- string(data)
	}()
	runErr := fn()
	os.Stdout = previous
	_ = w.Close()
	return <-done, runErr
}

// transcriptMarker stands for the provider transcript. The formatted review
// must replace it, not follow it.
const transcriptMarker = "TRANSCRIPT-LINE-thinking about the code"

const jsonAnswerLog = transcriptMarker + "\n" + `{"summary": "One bug.", "findings": [{"file": "a.go", "line": 3, "severity": "high", "category": "bug", "title": "nil deref", "body": "x is nil", "suggestion": "check x", "confidence": 9}]}` + "\n"

func runCommandWith(t *testing.T, f *fakeRun, input, workdir string) (string, error) {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	withStdin(t, input)
	return captureStdout(t, func() error { return runModelCommand(f.spec(), workdir, true) })
}

func TestCommandReviewPrintsFormattedReview(t *testing.T) {
	f := &fakeRun{log: jsonAnswerLog}
	out, err := runCommandWith(t, f, "review src/", t.TempDir())
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if !f.review || !strings.Contains(f.prompt, "## Role: Implementation Bug Hunter") || !strings.Contains(f.prompt, "Review scope: src/") {
		t.Errorf("provider did not get the bug-hunter review prompt:\n%.300s", f.prompt)
	}
	for _, want := range []string{"═══ RIVAL REVIEW ═══", "Model: codex (gpt-6-astra)", "Scope: src/", "1. [high] nil deref — a.go:3", "\nLog: "} {
		if !strings.Contains(out, want) {
			t.Errorf("stdout missing %q:\n%s", want, out)
		}
	}
	if strings.Contains(out, transcriptMarker) {
		t.Errorf("the transcript was printed:\n%s", out)
	}
}

func TestCommandReviewProseFallsBackToLog(t *testing.T) {
	f := &fakeRun{log: "I looked around and everything seems fine to me.\n"}
	out, err := runCommandWith(t, f, "review src/", t.TempDir())
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	for _, want := range []string{"RIVAL REVIEW — UNPARSED OUTPUT", "everything seems fine to me", "\nLog: "} {
		if !strings.Contains(out, want) {
			t.Errorf("stdout missing %q:\n%s", want, out)
		}
	}
}

func TestCommandRawPromptPrintsLog(t *testing.T) {
	f := &fakeRun{log: "the auth flow works like this\n"}
	out, err := runCommandWith(t, f, "explain the auth flow", t.TempDir())
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if f.review || f.prompt != "explain the auth flow" {
		t.Errorf("raw prompt changed: review=%v prompt=%q", f.review, f.prompt)
	}
	if out != "the auth flow works like this\n" {
		t.Errorf("raw prompt output = %q, want the log verbatim", out)
	}
}

func TestCommandReviewFailureKeepsLogAndExitCode(t *testing.T) {
	f := &fakeRun{log: jsonAnswerLog, exitCode: 2}
	out, err := runCommandWith(t, f, "review src/", t.TempDir())
	var exitErr *ExitCodeError
	if !errors.As(err, &exitErr) || exitErr.Code != 2 {
		t.Fatalf("err = %v, want ExitCodeError code 2", err)
	}
	if !strings.Contains(out, transcriptMarker) || strings.Contains(out, "RIVAL REVIEW") {
		t.Errorf("failed run must print the log, not a formatted review:\n%s", out)
	}
}

const testMRURL = "https://gitlab.example.com/team/app/-/merge_requests/42"

// fakeMR replaces the MR resolver with a snapshot in a temp dir.
func fakeMR(t *testing.T) (snapshotDir string, calls *int) {
	t.Helper()
	snapshotDir = filepath.Join(t.TempDir(), "rival-mr-fake")
	if err := os.Mkdir(snapshotDir, 0o700); err != nil {
		t.Fatal(err)
	}
	calls = new(int)
	previous := prepareMR
	prepareMR = func(_ context.Context, scope, _ string) (*mergerequest.Snapshot, error) {
		*calls++
		if !mergerequest.Contains(scope) {
			return nil, nil
		}
		return &mergerequest.Snapshot{
			Workdir:  snapshotDir,
			Scope:    "PINNED-SNAPSHOT-SCOPE with the patch",
			Identity: "GitLab MR: " + testMRURL,
		}, nil
	}
	t.Cleanup(func() { prepareMR = previous })
	return snapshotDir, calls
}

func TestCommandMRReviewRunsInSnapshot(t *testing.T) {
	snapshotDir, calls := fakeMR(t)
	callerDir := t.TempDir()
	f := &fakeRun{log: jsonAnswerLog}
	out, err := runCommandWith(t, f, "review "+testMRURL, callerDir)
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if *calls != 1 {
		t.Fatalf("MR resolver called %d times, want 1", *calls)
	}
	if f.workdir != snapshotDir || !f.workdirExisted {
		t.Errorf("run workdir = %q (existed %v), want the live snapshot %q", f.workdir, f.workdirExisted, snapshotDir)
	}
	if f.credWorkdir != callerDir {
		t.Errorf("credential workdir = %q, want the caller's %q", f.credWorkdir, callerDir)
	}
	if !strings.Contains(f.prompt, "Review scope: PINNED-SNAPSHOT-SCOPE") {
		t.Errorf("prompt does not carry the snapshot scope:\n%.300s", f.prompt)
	}
	if !strings.HasPrefix(out, "GitLab MR: "+testMRURL+"\n") {
		t.Errorf("identity line is not first:\n%s", out)
	}
	if !strings.Contains(out, "Scope: "+testMRURL+"\n") {
		t.Errorf("formatted review should show the MR URL as scope:\n%s", out)
	}
	if _, err := os.Stat(snapshotDir); !os.IsNotExist(err) {
		t.Errorf("MR checkout was not closed after the run (stat err %v)", err)
	}
}

func TestCommandPlainScopeSkipsMRResolver(t *testing.T) {
	_, calls := fakeMR(t)
	callerDir := t.TempDir()
	f := &fakeRun{log: jsonAnswerLog}
	if _, err := runCommandWith(t, f, "review src/", callerDir); err != nil {
		t.Fatalf("run: %v", err)
	}
	if *calls != 0 || f.workdir != callerDir || f.credWorkdir != callerDir {
		t.Errorf("plain scope changed: resolver calls %d, workdir %q, cred %q", *calls, f.workdir, f.credWorkdir)
	}
}

func TestRunMRReviewRunsInSnapshot(t *testing.T) {
	snapshotDir, _ := fakeMR(t)
	t.Setenv("HOME", t.TempDir())
	callerDir := t.TempDir()
	f := &fakeRun{log: jsonAnswerLog}
	out, err := captureStdout(t, func() error {
		return runModelRun(f.spec(), runOptions{workdir: callerDir, noQueue: true, reviewScope: testMRURL, isReview: true})
	})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if f.workdir != snapshotDir || f.credWorkdir != callerDir || !strings.Contains(f.prompt, "PINNED-SNAPSHOT-SCOPE") {
		t.Errorf("run surface did not use the snapshot: workdir %q cred %q", f.workdir, f.credWorkdir)
	}
	if !strings.HasPrefix(out, "GitLab MR: ") || !strings.Contains(out, "═══ RIVAL REVIEW ═══") {
		t.Errorf("run output missing identity or formatted review:\n%s", out)
	}
	if _, err := os.Stat(snapshotDir); !os.IsNotExist(err) {
		t.Error("MR checkout was not closed after the run")
	}
}

// A zero exit is not a review. The removed megareview runner failed a run
// that only hit a quota or wrote nothing; single-model review (MR reviews
// included) must too. Found by the 2026-09-26 branch review.
func TestCommandReviewQuotaOrEmptyFailsTheRun(t *testing.T) {
	for name, log := range map[string]string{
		"quota": "ERROR: insufficient_quota: You exceeded your current quota\n",
		"empty": "",
	} {
		t.Run(name, func(t *testing.T) {
			f := &fakeRun{log: log}
			out, err := runCommandWith(t, f, "review src/", t.TempDir())
			var exitErr *ExitCodeError
			if !errors.As(err, &exitErr) || exitErr.Code != 1 {
				t.Fatalf("err = %v, want ExitCodeError code 1", err)
			}
			if strings.Contains(out, "═══ RIVAL REVIEW ═══") {
				t.Errorf("a failed run printed a review:\n%s", out)
			}
		})
	}
}

// `rival run <model> --review` with no scope auto-detects like command mode;
// outside a git repo that falls back to the whole project.
func TestRunReviewEmptyScopeAutoDetects(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	f := &fakeRun{log: jsonAnswerLog}
	out, err := captureStdout(t, func() error {
		return runModelRun(f.spec(), runOptions{workdir: t.TempDir(), noQueue: true, isReview: true})
	})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if !strings.Contains(f.prompt, "Review scope: the entire project") {
		t.Errorf("empty scope did not fall back to the whole project:\n%.300s", f.prompt)
	}
	if !strings.Contains(out, "Scope: the entire project\n") {
		t.Errorf("formatted review scope wrong:\n%s", out)
	}
}
