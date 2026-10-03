//! Go: `internal/executor/claude_docker.go`.

#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::Path;

use anyhow::bail;

use super::claude::claude_args;
use super::oscmd::{self, Output};
use super::process;
use super::subprocess::{Request, RunResult, io_text};
use crate::config::{self, Config};
use crate::gostd::quote;
use crate::logging;
use crate::session::Session;

const CLAUDE_DOCKER_IMAGE: &str = config::CLAUDE_DOCKER_IMAGE;

/// Embedded Dockerfile content — written to a temp file for auto-build.
const CLAUDE_DOCKERFILE: &str = "FROM node:22-slim
RUN npm install -g @anthropic-ai/claude-code && \\
    useradd -m -s /bin/bash claude
USER claude
WORKDIR /workspace
ENTRYPOINT [\"claude\"]
";

/// Go `ClaudeDockerPreflight`: checks docker is available, the token is set,
/// and the image exists (auto-builds it if missing).
pub fn claude_docker_preflight(cfg: &Config) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "docker").is_err() {
        bail!("claude runtime requires Docker but docker is not installed");
    }

    // Check the docker daemon is running.
    if oscmd::run(cfg, "docker", &["info"], Output::Discard)
        .1
        .is_err()
    {
        bail!(
            "claude runtime requires Docker but the daemon is not running — start Docker Desktop and retry"
        );
    }

    let token = cfg.getenv(config::CLAUDE_DOCKER_TOKEN_ENV);
    if token.is_empty() {
        let env = config::CLAUDE_DOCKER_TOKEN_ENV;
        bail!(
            "{env} env var not set. To authenticate:\n  1. docker run -d --name rival-claude-login --user claude --entrypoint sh {CLAUDE_DOCKER_IMAGE} -c 'sleep 3600'\n  2. docker exec -it rival-claude-login claude login\n  3. docker exec rival-claude-login cat /home/claude/.claude/.credentials.json\n  4. export {env}=<accessToken from step 3>\n  5. docker rm -f rival-claude-login"
        );
    }

    // Check the image exists, auto-build if missing.
    let inspect = oscmd::run(
        cfg,
        "docker",
        &["image", "inspect", CLAUDE_DOCKER_IMAGE],
        Output::Discard,
    );
    if inspect.1.is_err() {
        logging::info().msg("Claude Docker image not found, building...");
        if let Err(build_err) = build_claude_docker_image(cfg) {
            bail!("failed to build {CLAUDE_DOCKER_IMAGE} docker image: {build_err}");
        }
        logging::info().msg("Claude Docker image built successfully");
    }

    Ok(())
}

/// Go `buildClaudeDockerImage`. The error is Go's wrapped text.
fn build_claude_docker_image(cfg: &Config) -> Result<(), String> {
    // Write the embedded Dockerfile to a temp file.
    let (mut file, name) = oscmd::create_temp(cfg, "rival-claude-dockerfile-*")
        .map_err(|e| format!("create temp dockerfile: {e}"))?;
    let _remove = RemoveOnDrop(&name);

    if let Err(e) = file.write_all(CLAUDE_DOCKERFILE.as_bytes()) {
        let _ = process::close_file(file);
        return Err(format!("write dockerfile: write {name}: {}", io_text(&e)));
    }
    if let Err(e) = process::close_file(file) {
        return Err(format!("close dockerfile: close {name}: {}", io_text(&e)));
    }

    // Build the image; progress goes to stderr.
    let (_, result) = oscmd::run(
        cfg,
        "docker",
        &["build", "-t", CLAUDE_DOCKER_IMAGE, "-f", &name, "."],
        Output::Stderr,
    );
    result.map_err(|e| format!("docker build: {e}"))
}

/// Go's `defer os.Remove(name)` with the error dropped.
struct RemoveOnDrop<'a>(&'a str);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Go `runClaudeDocker`: executes Claude through the Claude Code CLI inside
/// Docker. `spawn` is the subprocess step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_claude_docker_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    read_only: bool,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    if model != config::CLAUDE_MODEL {
        bail!("unsupported Claude Code model {}", quote(model));
    }
    let token = cfg.getenv(config::CLAUDE_DOCKER_TOKEN_ENV);
    if token.is_empty() {
        bail!("{} env var not set", config::CLAUDE_DOCKER_TOKEN_ENV);
    }

    // The workdir must be absolute for the Docker volume mount. Go joins
    // with a bare "/" and does not clean the result. Go tests for a leading
    // "/" on every OS, so on Windows `C:\repo` becomes `<cwd>/C:\repo`: a
    // known Go quirk, kept as the source does it.
    let mut abs_workdir = workdir.to_string();
    if !abs_workdir.starts_with('/') {
        let Some(wd) = cfg.cwd() else {
            bail!("get working dir: getwd: no such file or directory");
        };
        abs_workdir = format!("{}/{workdir}", wd.to_string_lossy());
    }

    let mut mount = format!("{abs_workdir}:/workspace");
    if read_only {
        mount.push_str(":ro");
    }
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "-i",
        "-v",
        &mount,
        "-w",
        "/workspace",
        "-e",
        &format!("ANTHROPIC_AUTH_TOKEN={token}"),
        CLAUDE_DOCKER_IMAGE,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(claude_args(model, effort, read_only));

    let full_prompt = format!(
        "{}\n{prompt}",
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    let req = Request {
        binary: "docker",
        args: &args,
        env: &[],
        prompt: &full_prompt,
        drop_env: &[],
        environ: cfg.environ(),
    };
    spawn(sess, &req)
}
