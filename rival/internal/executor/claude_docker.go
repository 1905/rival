package executor

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

const claudeDockerImage = config.ClaudeDockerImage

// Embedded Dockerfile content — written to temp file for auto-build.
const claudeDockerfile = `FROM node:22-slim
RUN npm install -g @anthropic-ai/claude-code && \
    useradd -m -s /bin/bash claude
USER claude
WORKDIR /workspace
ENTRYPOINT ["claude"]
`

// ClaudeDockerPreflight checks docker is available, token is set, and image exists (auto-builds if missing).
func ClaudeDockerPreflight() error {
	if _, err := exec.LookPath("docker"); err != nil {
		return fmt.Errorf("claude runtime requires Docker but docker is not installed")
	}

	// Check docker daemon is running.
	cmd := exec.Command("docker", "info")
	cmd.Stdout = nil
	cmd.Stderr = nil
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("claude runtime requires Docker but the daemon is not running — start Docker Desktop and retry")
	}

	token := os.Getenv(config.ClaudeDockerTokenEnv)
	if token == "" {
		return fmt.Errorf("%s env var not set. To authenticate:\n"+
			"  1. docker run -d --name rival-claude-login --user claude --entrypoint sh %s -c 'sleep 3600'\n"+
			"  2. docker exec -it rival-claude-login claude login\n"+
			"  3. docker exec rival-claude-login cat /home/claude/.claude/.credentials.json\n"+
			"  4. export %s=<accessToken from step 3>\n"+
			"  5. docker rm -f rival-claude-login",
			config.ClaudeDockerTokenEnv, claudeDockerImage, config.ClaudeDockerTokenEnv)
	}

	// Check image exists, auto-build if missing.
	inspectCmd := exec.Command("docker", "image", "inspect", claudeDockerImage)
	inspectCmd.Stdout = nil
	inspectCmd.Stderr = nil
	if err := inspectCmd.Run(); err != nil {
		log.Info().Msg("Claude Docker image not found, building...")
		if buildErr := buildClaudeDockerImage(); buildErr != nil {
			return fmt.Errorf("failed to build %s docker image: %w", claudeDockerImage, buildErr)
		}
		log.Info().Msg("Claude Docker image built successfully")
	}

	return nil
}

func buildClaudeDockerImage() error {
	// Write embedded Dockerfile to temp file.
	tmpFile, err := os.CreateTemp("", "rival-claude-dockerfile-*")
	if err != nil {
		return fmt.Errorf("create temp dockerfile: %w", err)
	}
	defer func() { _ = os.Remove(tmpFile.Name()) }()

	if _, err := tmpFile.WriteString(claudeDockerfile); err != nil {
		_ = tmpFile.Close()
		return fmt.Errorf("write dockerfile: %w", err)
	}
	if err := tmpFile.Close(); err != nil {
		return fmt.Errorf("close dockerfile: %w", err)
	}

	// Build image.
	cmd := exec.Command("docker", "build", "-t", claudeDockerImage, "-f", tmpFile.Name(), ".")
	cmd.Stdout = os.Stderr // Show build progress on stderr.
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("docker build: %w", err)
	}
	return nil
}

// runClaudeDocker executes Claude through the Claude Code CLI inside Docker.
func runClaudeDocker(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, readOnly bool, mirror io.Writer) (*Result, error) {
	if model != config.ClaudeModel {
		return nil, fmt.Errorf("unsupported Claude Code model %q", model)
	}
	token := os.Getenv(config.ClaudeDockerTokenEnv)
	if token == "" {
		return nil, fmt.Errorf("%s env var not set", config.ClaudeDockerTokenEnv)
	}

	// Ensure workdir is absolute for Docker volume mount.
	absWorkdir := workdir
	if !strings.HasPrefix(absWorkdir, "/") {
		wd, err := os.Getwd()
		if err != nil {
			return nil, fmt.Errorf("get working dir: %w", err)
		}
		absWorkdir = wd + "/" + workdir
	}

	mount := absWorkdir + ":/workspace"
	if readOnly {
		mount += ":ro"
	}
	args := []string{
		"run", "--rm", "-i",
		"-v", mount,
		"-w", "/workspace",
		"-e", "ANTHROPIC_AUTH_TOKEN=" + token,
		claudeDockerImage,
	}
	args = append(args, claudeArgs(model, effort, readOnly)...)

	fullPrompt := config.BuildWorkdirPreamble(workdir) + "\n" + prompt
	return RunSubprocess(ctx, sess, "docker", args, nil, fullPrompt, mirror)
}
