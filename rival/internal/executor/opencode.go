package executor

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
)

// OpencodePreflightModel validates K3, Rival's sole OpenCode-backed model.
// workdir seeds
// the Moonshot API-key .env walk-up for K3 (see config.KimiAPIKeyFrom);
// pass "" when no workdir context exists.
func OpencodePreflightModel(model, workdir string) error {
	entry, ok := config.OpenCodeEntryFor(model)
	if !ok {
		return fmt.Errorf("unsupported OpenCode model %q", model)
	}
	return OpencodePreflightEntry(entry, workdir)
}

// OpencodePreflightEntry verifies one registry entry can run: the CLI exists
// and its credential resolves. The two failures are reported separately,
// because a present key does not help when the binary is missing.
func OpencodePreflightEntry(entry config.SecurityModel, workdir string) error {
	if _, err := exec.LookPath("opencode"); err != nil {
		return fmt.Errorf("opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash")
	}
	if config.SecurityAPIKeyFrom(entry, workdir) == "" {
		return fmt.Errorf("model %s requires %s — add it to the project .env or export it", entry.Label, entry.KeyEnv)
	}
	return nil
}

// opencodeReadOnlyPermission is a read-only, workdir-scoped permission profile
// passed to opencode via OPENCODE_PERMISSION. A code reviewer reads repo content
// that may contain prompt-injection, so it must NOT write files, run shell
// commands, OR read outside the reviewed workdir. read/grep/glob/list are allowed
// (opencode auto-scopes these to the workdir + its own tool-output dirs);
// external_directory is DENIED so a prompt-injected repo can't make the reviewer
// read host secrets (~/.aws/credentials, ~/.ssh, a sibling repo's .env) and
// exfiltrate them through the review output / logs / consilium prompt. edit/bash/
// task and web access are denied. Verified: in-workdir reads work,
// out-of-workdir reads are blocked by the external_directory deny rule.
const opencodeReadOnlyPermission = `{"read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny","edit":"deny","bash":"deny","task":"deny","webfetch":"deny","websearch":"deny"}`

// opencodeFullAutoPermission allows every tool except out-of-workdir native
// reads. Used only by the standalone kimi runner's raw-prompt mode, where the
// user explicitly asked for a full-auto agent that can edit files and run
// commands in the workdir. external_directory is denied to keep the native
// file tools on the documented "in the workdir" promise — bash being allowed
// means this is defense-in-depth, not containment (a shell can read anything
// the user can). Review mode never uses this profile.
const opencodeFullAutoPermission = `{"read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny","edit":"allow","bash":"allow","task":"allow","webfetch":"allow","websearch":"allow"}`

// OpencodeRunOpts customizes one opencode execution beyond the reviewer
// defaults. Zero values keep megareview behavior exactly: read-only
// permission, Moonshot key injection, no extra env drops.
type OpencodeRunOpts struct {
	Permission string   // OPENCODE_PERMISSION JSON; "" = read-only reviewer profile
	APIKey     string   // Moonshot API key; "" = config.KimiAPIKeyFrom(workdir)
	DropEnv    []string // extra vars/prefixes stripped from the child (see dropMatches)
}

// RunOpencode executes a K3 prompt through the opencode CLI. The prompt is read from stdin in
// non-interactive `run` mode; the effort is mapped to opencode's --variant
// (provider-specific reasoning level). It runs under a read-only permission
// profile (see opencodeReadOnlyPermission) rather than --dangerously-skip-permissions,
// so a prompt-injected repo cannot make the reviewer write files or run commands.
// An empty model falls back to K3.
func RunOpencode(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, mirror io.Writer) (*Result, error) {
	return RunOpencodeWith(ctx, sess, prompt, effort, workdir, model, OpencodeRunOpts{}, mirror)
}

// RunOpencodeWith is RunOpencode with per-call overrides (see OpencodeRunOpts).
// The standalone kimi runner uses it for its full-auto raw mode and its
// moonshot-provider key; megareview reviewers stay on the zero-value defaults.
func RunOpencodeWith(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, opts OpencodeRunOpts, mirror io.Writer) (*Result, error) {
	if model == "" {
		model = config.KimiModel
	}
	entry, ok := config.OpenCodeEntryFor(model)
	if !ok {
		return nil, fmt.Errorf("unsupported OpenCode model %q", model)
	}
	return RunOpencodeEntry(ctx, sess, prompt, effort, workdir, entry, opts, mirror)
}

// RunOpencodeEntry runs one registry entry. Everything provider-specific —
// the -m selector, the config block, the credential, the reasoning variant —
// comes from the entry, so adding a model is a registry change rather than a
// code change.
func RunOpencodeEntry(ctx context.Context, sess *session.Session, prompt, effort, workdir string, entry config.SecurityModel, opts OpencodeRunOpts, mirror io.Writer) (*Result, error) {
	args := opencodeRunArgs(entry, effort, workdir)
	env := opencodeRunEnvWith(sess.ID, entry, workdir, opts)

	fullPrompt := config.SystemPrompt + "\n\n" + config.BuildWorkdirPreamble(workdir) + "\n" + prompt
	// Drop any inherited OPENCODE_PERMISSION / OPENCODE_CONFIG_CONTENT before
	// appending ours. rival loads the reviewed repo's .env (godotenv) into the
	// process env, so a malicious repo could otherwise ship a permissive
	// OPENCODE_PERMISSION or a config that weakens the sandbox. (safeEnv already
	// strips the OPENCODE_ prefix, so this is belt-and-suspenders.)
	drop := append([]string{"OPENCODE_PERMISSION", "OPENCODE_CONFIG_CONTENT", "OPENCODE_DB"}, opts.DropEnv...)
	return RunSubprocess(ctx, sess, "opencode", args, env, fullPrompt, mirror, drop...)
}

func opencodeRunArgs(entry config.SecurityModel, effort, workdir string) []string {
	args := []string{
		"run",
		// --pure runs without external plugins / project-controlled config, so a
		// reviewed repo's own .opencode config can't re-enable denied tools or
		// otherwise weaken the read-only sandbox. (OPENCODE_PERMISSION already
		// wins over project config, but this removes all reliance on that.)
		"--pure",
		// OpenCode splits this at the first slash to choose the provider, so
		// an OpenRouter-hosted model needs the openrouter/ prefix here even
		// though its upstream id does not carry one.
		"-m", entry.Selector,
	}
	if variant := entry.Variant; variant != "" {
		args = append(args, "--variant", variant)
	}
	args = append(args, "--dir", workdir)
	return args
}

func opencodeRunEnvWith(sessionID string, entry config.SecurityModel, workdir string, opts OpencodeRunOpts) []string {
	permission := opts.Permission
	if permission == "" {
		permission = opencodeReadOnlyPermission
	}
	env := []string{
		"OPENCODE_PERMISSION=" + permission,
		// Give each reviewer its OWN opencode session DB. The megareview runs
		// several opencode processes at once and they otherwise share one SQLite
		// DB (WAL + 5s busy_timeout), which intermittently loses the write lock —
		// observed as a reviewer failing with "database is locked" (exit 1). A
		// per-session DB (keyed on the unique session ID) removes all contention.
		"OPENCODE_DB=rival-" + sessionID + ".db",
	}

	// Inject the entry's key into its provider config. A caller may supply an
	// explicit key; otherwise resolve the entry's own variable from the
	// process env or the workdir .env walk-up. The key is never hardcoded or
	// written to disk.
	key := opts.APIKey
	if key == "" {
		key = config.SecurityAPIKeyFrom(entry, workdir)
	}
	if key != "" {
		if cfg := opencodeProviderConfig(entry, key); cfg != "" {
			env = append(env, "OPENCODE_CONFIG_CONTENT="+cfg)
		}
	}
	return env
}

// opencodeProviderConfig returns the in-memory provider config for one
// registry entry. An empty key is rejected.
func opencodeProviderConfig(entry config.SecurityModel, key string) string {
	if key == "" {
		return ""
	}
	options := map[string]any{"apiKey": key}
	if entry.BaseURL != "" {
		options["baseURL"] = entry.BaseURL
	}
	cfg := map[string]any{
		"$schema": "https://opencode.ai/config.json",
		"provider": map[string]any{
			entry.Provider: map[string]any{
				"options": options,
			},
		},
	}
	b, err := json.Marshal(cfg)
	if err != nil {
		return ""
	}
	return string(b)
}
