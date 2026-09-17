package mergerequest

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const testURL = "https://gitlab.example.com/group/sub/app/-/merge_requests/42"

func TestParseTarget(t *testing.T) {
	for _, suffix := range []string{"", "/", "/diffs", "/commits", "?foo=bar#note_123"} {
		got, err := parseTarget(testURL + suffix)
		if err != nil || got.URL != testURL || got.Project != "group/sub/app" || got.IID != 42 {
			t.Fatalf("parse %q = %+v, %v", suffix, got, err)
		}
	}
	for _, raw := range []string{
		"review " + testURL, testURL + " " + testURL,
		strings.Replace(testURL, "https:", "http:", 1),
		strings.Replace(testURL, "https://", "https://token@", 1),
		strings.Replace(testURL, "/42", "/0", 1), testURL + "/unknown",
		strings.Replace(testURL, "/42", "/NaN", 1),
		strings.Replace(testURL, "group/sub/app", "group/../app", 1),
		"https://gitlab.example.com/app/-/merge_requests/42",
	} {
		if _, err := parseTarget(raw); err == nil {
			t.Errorf("accepted invalid MR target %q", raw)
		}
	}
}

func TestRemoteIdentity(t *testing.T) {
	for _, raw := range []string{
		"git@gitlab.example.com:group/sub/app.git",
		"ssh://git@gitlab.example.com/group/sub/app.git",
		"ssh://git@gitlab.example.com:2222/group/sub/app.git",
		"https://gitlab.example.com/group/sub/app.git",
	} {
		host, project := remoteIdentity(raw)
		if host != "gitlab.example.com" || project != "group/sub/app" {
			t.Errorf("%s = %q/%q", raw, host, project)
		}
	}
	for _, raw := range []string{"/tmp/repo", "file:///tmp/repo", "https://token@gitlab.example.com/group/sub/app.git"} {
		if host, _ := remoteIdentity(raw); host != "" {
			t.Errorf("accepted local or credential-bearing remote %q", raw)
		}
	}
}

func TestPrepareKeepsOrdinaryScopesLocal(t *testing.T) {
	snapshot, err := Prepare(context.Background(), "src/api/", "/does-not-exist")
	if err != nil || snapshot != nil {
		t.Fatalf("local scope = %+v, %v", snapshot, err)
	}
}

func TestPreparePinsMRWithoutChangingDirtyCheckout(t *testing.T) {
	f := newFixture(t)
	before := f.git(f.workdir, "status", "--porcelain")
	head := f.git(f.workdir, "rev-parse", "HEAD")
	snapshot, err := Prepare(context.Background(), testURL+"/diffs#note_123", f.workdir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = snapshot.Close() })
	if got := f.git(snapshot.Workdir, "rev-parse", "HEAD"); got != f.mr.SHA {
		t.Fatalf("reviewed %s, wanted %s", got, f.mr.SHA)
	}
	if got := read(t, filepath.Join(snapshot.Workdir, "code.txt")); got != "MR content\n" {
		t.Fatalf("wrong review content: %q", got)
	}
	if _, err := os.Stat(filepath.Join(snapshot.Workdir, "untracked.txt")); !os.IsNotExist(err) {
		t.Fatal("copied an unrelated untracked file into the review")
	}
	if got := f.git(f.workdir, "status", "--porcelain"); got != before || f.git(f.workdir, "rev-parse", "HEAD") != head {
		t.Fatal("changed the caller's branch, index, or working tree")
	}
	if diff := f.git(snapshot.Workdir, "diff", f.mr.DiffRefs.Base, f.mr.DiffRefs.Head, "--"); !strings.Contains(diff, "+MR content") || strings.Contains(diff, "unrelated") {
		t.Fatalf("wrong MR diff: %s", diff)
	}
	if !strings.Contains(snapshot.Identity, f.mr.SHA) || !strings.Contains(snapshot.Scope, f.mr.DiffRefs.Base) {
		t.Fatal("report/prompt omit immutable review identity")
	}
	if !strings.Contains(snapshot.Scope, "+MR content") || strings.Contains(snapshot.Scope, "+unrelated commit") {
		t.Fatal("shell-denied reviewers did not receive the exact MR patch")
	}
	if got := read(t, f.apiArgs); got != "api\n--hostname\ngitlab.example.com\nprojects/group%2Fsub%2Fapp/merge_requests/42\n" {
		t.Fatalf("API request did not use URL host/project: %q", got)
	}
	if err := snapshot.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(snapshot.Workdir); !os.IsNotExist(err) {
		t.Fatal("snapshot was not removed")
	}
}

func TestPrepareIgnoresInheritedGitRepository(t *testing.T) {
	for _, name := range []string{"GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t)
			before := f.git(f.workdir, "status", "--porcelain")
			head := f.git(f.workdir, "rev-parse", "HEAD")
			indexPath := filepath.Join(f.workdir, ".git", "index")
			index := read(t, indexPath)
			value := f.workdir
			if name == "GIT_DIR" {
				value = filepath.Join(f.workdir, ".git")
			} else if name == "GIT_INDEX_FILE" {
				value = indexPath
			}
			t.Setenv(name, value)
			snapshot, err := Prepare(context.Background(), testURL, f.workdir)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = snapshot.Close() })
			if err := os.Unsetenv(name); err != nil {
				t.Fatal(err)
			}
			if got := f.git(snapshot.Workdir, "rev-parse", "HEAD"); got != f.mr.SHA {
				t.Fatalf("snapshot HEAD = %s, want %s", got, f.mr.SHA)
			}
			if read(t, indexPath) != index || f.git(f.workdir, "status", "--porcelain") != before || f.git(f.workdir, "rev-parse", "HEAD") != head {
				t.Fatal("MR preparation changed the caller's index, files, or HEAD")
			}
		})
	}
}

func TestPrepareRefusesOversizedDiff(t *testing.T) {
	f := newFixture(t)
	remote := os.Getenv("MR_REMOTE")
	write(t, filepath.Join(remote, "code.txt"), strings.Repeat("added line\n", maxDiffBytes/10+1), 0o600)
	f.git(remote, "commit", "-qam", "large patch")
	f.mr.SHA = f.git(remote, "rev-parse", "HEAD")
	f.mr.DiffRefs.Head = f.mr.SHA
	f.save()
	snapshot, err := Prepare(context.Background(), testURL, f.workdir)
	if snapshot != nil || err == nil || !strings.Contains(err.Error(), "refusing a truncated review") {
		t.Fatalf("oversized diff: snapshot=%v err=%v", snapshot, err)
	}
	entries, err := os.ReadDir(f.snapshots)
	if err != nil || len(entries) != 0 {
		t.Fatalf("leaked oversized checkout: %v, %v", entries, err)
	}
}

func TestPrepareFailsClosed(t *testing.T) {
	for _, name := range []string{"api", "json", "wrong-mr", "wrong-project", "missing-base", "stale-head", "missing-object", "wrong-remote", "fork-with-target-remote", "cancelled"} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t)
			ctx := context.Background()
			switch name {
			case "api":
				t.Setenv("MR_API_FAIL", "1")
			case "wrong-mr":
				f.mr.IID++
			case "wrong-project":
				f.mr.WebURL = strings.Replace(testURL, "/app/", "/different/", 1)
			case "missing-base":
				f.mr.DiffRefs.Base = ""
			case "stale-head":
				f.mr.SHA = f.mr.DiffRefs.Base
			case "missing-object":
				f.mr.DiffRefs.Base = strings.Repeat("a", 40)
			case "wrong-remote", "fork-with-target-remote":
				f.git(f.workdir, "remote", "set-url", "origin", "https://gitlab.example.com/fork/app.git")
				if name == "fork-with-target-remote" {
					f.git(f.workdir, "remote", "add", "upstream", "https://gitlab.example.com/group/sub/app.git")
				}
			case "cancelled":
				var cancel context.CancelFunc
				ctx, cancel = context.WithCancel(ctx)
				cancel()
			}
			f.save()
			if name == "json" {
				write(t, f.metaPath, "not JSON", 0o600)
			}
			snapshot, err := Prepare(ctx, testURL, f.workdir)
			if name == "fork-with-target-remote" {
				if err != nil {
					t.Fatal(err)
				}
				_ = snapshot.Close()
			} else if err == nil || snapshot != nil {
				t.Fatalf("expected refusal, got snapshot=%+v error=%v", snapshot, err)
			}
			entries, err := os.ReadDir(f.snapshots)
			if err != nil || len(entries) != 0 {
				t.Fatalf("leaked checkout after completion/failure: %v, %v", entries, err)
			}
		})
	}
}

type fixture struct {
	t                                              *testing.T
	workdir, metaPath, apiArgs, snapshots, gitPath string
	mr                                             metadata
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	root := t.TempDir()
	realGit, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	f := &fixture{t: t, gitPath: realGit, workdir: filepath.Join(root, "work"), metaPath: filepath.Join(root, "mr.json"), apiArgs: filepath.Join(root, "api-args"), snapshots: filepath.Join(root, "snapshots")}
	remote := filepath.Join(root, "remote")
	for _, dir := range []string{remote, f.snapshots, filepath.Join(root, "bin")} {
		if err := os.Mkdir(dir, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	// Avoid host Git config, hooks, signing, aliases, and credentials in tests.
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_AUTHOR_NAME", "Test")
	t.Setenv("GIT_COMMITTER_NAME", "Test")
	t.Setenv("GIT_AUTHOR_EMAIL", "test@example.com")
	t.Setenv("GIT_COMMITTER_EMAIL", "test@example.com")
	f.git(remote, "init", "--quiet")
	write(t, filepath.Join(remote, "code.txt"), "base\n", 0o600)
	f.git(remote, "add", ".")
	f.git(remote, "commit", "-qm", "base")
	f.mr.DiffRefs.Base = f.git(remote, "rev-parse", "HEAD")
	write(t, filepath.Join(remote, "code.txt"), "MR content\n", 0o600)
	f.git(remote, "commit", "-qam", "MR head")
	f.mr.SHA = f.git(remote, "rev-parse", "HEAD")
	f.mr.DiffRefs.Head = f.mr.SHA
	f.mr.IID, f.mr.WebURL = 42, testURL
	f.mr.SourceBranch, f.mr.TargetBranch = "feature", "main"
	f.save()
	f.git(root, "clone", "--quiet", remote, f.workdir)
	f.git(f.workdir, "checkout", "-q", "-b", "unrelated", f.mr.DiffRefs.Base)
	write(t, filepath.Join(f.workdir, "code.txt"), "unrelated commit\n", 0o600)
	f.git(f.workdir, "commit", "-qam", "unrelated")
	write(t, filepath.Join(f.workdir, "code.txt"), "dirty unrelated file\n", 0o600)
	write(t, filepath.Join(f.workdir, "untracked.txt"), "untracked\n", 0o600)
	f.git(f.workdir, "remote", "set-url", "origin", "https://gitlab.example.com/group/sub/app.git")
	// Only the network transport is replaced. All Git operations and objects
	// are real; the fake glab supplies a fixed API response without credentials.
	write(t, filepath.Join(root, "bin", "git"), `#!/bin/sh
if [ "$1" = fetch ]; then
  exec "$MR_REAL_GIT" -c "url.$MR_REMOTE.insteadOf=https://gitlab.example.com/group/sub/app.git" "$@"
fi
exec "$MR_REAL_GIT" "$@"
`, 0o700)
	write(t, filepath.Join(root, "bin", "glab"), `#!/bin/sh
[ -z "$GITLAB_TOKEN$GITLAB_ACCESS_TOKEN$OAUTH_TOKEN$CI_JOB_TOKEN$GITLAB_HOST" ] || exit 90
printf '%s\n' "$@" > "$MR_API_ARGS"
[ "$MR_API_FAIL" != 1 ] || exit 1
cat "$MR_METADATA"
`, 0o700)
	t.Setenv("PATH", filepath.Join(root, "bin")+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("MR_REAL_GIT", realGit)
	t.Setenv("MR_REMOTE", remote)
	t.Setenv("MR_METADATA", f.metaPath)
	t.Setenv("MR_API_ARGS", f.apiArgs)
	t.Setenv("MR_API_FAIL", "")
	t.Setenv("GITLAB_TOKEN", "wrong-host-token")
	t.Setenv("GITLAB_ACCESS_TOKEN", "wrong-host-token")
	t.Setenv("OAUTH_TOKEN", "wrong-host-token")
	t.Setenv("CI_JOB_TOKEN", "wrong-host-token")
	t.Setenv("GITLAB_HOST", "other.example.com")
	t.Setenv("TMPDIR", f.snapshots)
	return f
}

func (f *fixture) git(dir string, args ...string) string {
	f.t.Helper()
	cmd := exec.Command(f.gitPath, args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		f.t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func (f *fixture) save() {
	f.t.Helper()
	raw, err := json.Marshal(f.mr)
	if err != nil {
		f.t.Fatal(err)
	}
	write(f.t, f.metaPath, string(raw), 0o600)
}

func write(t *testing.T, path, data string, mode os.FileMode) {
	t.Helper()
	if err := os.WriteFile(path, []byte(data), mode); err != nil {
		t.Fatal(err)
	}
}

func read(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}
