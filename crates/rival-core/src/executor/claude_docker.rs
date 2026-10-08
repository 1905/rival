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

/// Checks docker is available, the token is set,
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

/// Builds the Docker image. The error is the wrapped text.
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

/// Removes the file on drop; a removal error is ignored.
struct RemoveOnDrop<'a>(&'a str);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Executes Claude through the Claude Code CLI inside
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
        bail!("unsupported Claude Code model {:?}", model);
    }
    let token = cfg.getenv(config::CLAUDE_DOCKER_TOKEN_ENV);
    if token.is_empty() {
        bail!("{} env var not set", config::CLAUDE_DOCKER_TOKEN_ENV);
    }

    // The workdir must be absolute for the Docker volume mount. A relative
    // path is joined with a bare "/" and the result is not cleaned. The test
    // is for a leading "/" on every OS, so on Windows `C:\repo` becomes
    // `<cwd>/C:\repo`: a known quirk, kept as is.
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
    // A known name lets a cancelled run remove its container: killing the
    // docker client does not stop the container it started.
    let name = container_name(&sess.id);
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "-i",
        "--name",
        &name,
        "-v",
        &mount,
        "-w",
        "/workspace",
        // The name only: Docker copies the value from its own environment.
        // A value here would show in `ps` to every local user.
        "-e",
        "ANTHROPIC_AUTH_TOKEN",
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
    let child_env = [format!("ANTHROPIC_AUTH_TOKEN={token}")];
    let req = Request {
        binary: "docker",
        args: &args,
        env: &child_env,
        prompt: &full_prompt,
        drop_env: &[],
        environ: cfg.environ(),
    };
    let result = spawn(sess, &req);
    // Exit 0 means the container ended and `--rm` removed it. Anything else
    // (a provider error, a timeout, a cancel) may leave it running against
    // the mounted project, so remove it before the queue slot is released.
    if !matches!(&result, Ok(r) if r.exit_code == 0) {
        remove_container(cfg, &name, REMOVE_TIMEOUT);
    }
    result
}

/// The container name for a session's Claude run.
pub(crate) fn container_name(session_id: &str) -> String {
    format!("rival-{session_id}")
}

/// How long `docker rm -f` may take. The caller still holds its queue
/// slot, so a hung Docker daemon must not hold it forever.
const REMOVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `docker rm -f <name>`, killed after `timeout`. A container `--rm`
/// already removed is not an error worth reporting.
fn remove_container(cfg: &Config, name: &str, timeout: std::time::Duration) {
    use std::io::Read;
    use std::process::Stdio;

    let warn = |e: &str| {
        logging::warn()
            .err(e)
            .str("container", name)
            .msg("could not remove the Claude container");
    };
    let mut cmd = match oscmd::command(cfg, "docker", &["rm", "-f", name]) {
        Ok((cmd, _)) => cmd,
        Err(e) => return warn(&e),
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = match process::spawn(&mut cmd) {
        Ok(child) => child,
        Err(e) => return warn(&io_text(&e)),
    };
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return warn(&format!("docker rm -f timed out after {timeout:?}"));
            }
            Err(e) => return warn(&io_text(&e)),
        }
    };
    if !status.success() {
        let mut text = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut text);
        }
        if !text.contains("No such container") {
            warn(&format!("{}: {}", status, text.trim()));
        }
    }
}
