//! The review mount is covered by
//! `claude::tests::claude_docker_review_mount_is_read_only`. These check the preflight
//! branches against a task-owned fake `docker` that never runs Docker.

use super::*;
use crate::executor::claude::claude_preflight;
use crate::executor::testutil::{Env, path_str, retry_busy};

/// A fake docker. `INFO`, `INSPECT` and `BUILD` are its exit codes. Each
/// call appends its argv to `calls`; a build also copies the `-f` file.
#[cfg(unix)]
fn fake_docker(env: &Env, info: i32, inspect: i32, build: i32) -> std::path::PathBuf {
    let calls = env.home.path().join("docker.calls");
    let copy = env.home.path().join("Dockerfile.seen");
    env.fake(
        "docker",
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{calls}'\ncase \"$1\" in\ninfo) exit {info} ;;\nimage) exit {inspect} ;;\nbuild) /bin/cp \"$5\" '{copy}'; exit {build} ;;\nesac\nexit 99\n",
            calls = path_str(&calls),
            copy = path_str(&copy),
        ),
    );
    calls
}

fn preflight(cfg: &Config) -> Result<(), String> {
    retry_busy(|| claude_docker_preflight(cfg), |r| format!("{r:?}")).map_err(|e| e.to_string())
}

#[test]
fn preflight_without_docker() {
    let env = Env::new();
    assert_eq!(
        preflight(&env.config()).unwrap_err(),
        "claude runtime requires Docker but docker is not installed"
    );
    // claude_preflight falls through to Docker when claude is absent.
    assert_eq!(
        claude_preflight(&env.config()).unwrap_err().to_string(),
        "claude runtime requires Docker but docker is not installed"
    );
}

#[cfg(unix)]
#[test]
fn claude_preflight_prefers_native_claude() {
    let env = Env::new();
    env.fake("claude", "#!/bin/sh\nexit 97\n");
    claude_preflight(&env.config()).unwrap();
}

#[cfg(unix)]
#[test]
fn preflight_reports_a_stopped_daemon() {
    let env = Env::new();
    fake_docker(&env, 1, 0, 0);
    assert_eq!(
        preflight(&env.config()).unwrap_err(),
        "claude runtime requires Docker but the daemon is not running — start Docker Desktop and retry"
    );
}

#[cfg(unix)]
#[test]
fn preflight_explains_a_missing_token() {
    let env = Env::new();
    let calls = fake_docker(&env, 0, 0, 0);
    assert_eq!(
        preflight(&env.config()).unwrap_err(),
        "RIVAL_CLAUDE_TOKEN env var not set. To authenticate:\n  1. docker run -d --name rival-claude-login --user claude --entrypoint sh rival-claude -c 'sleep 3600'\n  2. docker exec -it rival-claude-login claude login\n  3. docker exec rival-claude-login cat /home/claude/.claude/.credentials.json\n  4. export RIVAL_CLAUDE_TOKEN=<accessToken from step 3>\n  5. docker rm -f rival-claude-login"
    );
    assert_eq!(std::fs::read_to_string(calls).unwrap(), "info\n");
}

#[cfg(unix)]
#[test]
fn preflight_with_an_existing_image_does_not_build() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("tok"));
    let calls = fake_docker(&env, 0, 0, 1);
    preflight(&env.config()).unwrap();
    assert_eq!(
        std::fs::read_to_string(calls).unwrap(),
        "info\nimage inspect rival-claude\n"
    );
}

#[cfg(unix)]
#[test]
fn preflight_builds_a_missing_image_from_a_temp_dockerfile() {
    for (build_exit, want) in [
        (0, Ok(())),
        (
            3,
            Err(
                "failed to build rival-claude docker image: docker build: exit status: 3"
                    .to_string(),
            ),
        ),
    ] {
        let mut env = Env::new();
        let tmp = tempfile::tempdir().unwrap();
        env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("tok"))
            .set("TMPDIR", Some(&path_str(tmp.path())));
        let calls = fake_docker(&env, 0, 1, build_exit);
        assert_eq!(preflight(&env.config()), want);

        let calls = std::fs::read_to_string(calls).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines[..2], ["info", "image inspect rival-claude"]);
        let prefix = format!(
            "build -t rival-claude -f {}/rival-claude-dockerfile-",
            path_str(tmp.path())
        );
        let rest = lines[2].strip_prefix(&prefix).expect(lines[2]);
        let digits = rest.strip_suffix(" .").expect(rest);
        assert!(digits.bytes().all(|b| b.is_ascii_digit()), "{rest}");
        assert_eq!(lines.len(), 3);

        let seen = std::fs::read_to_string(env.home.path().join("Dockerfile.seen")).unwrap();
        assert_eq!(seen, CLAUDE_DOCKERFILE);
        // The temp Dockerfile is removed after the build.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}

#[test]
fn build_reports_a_temp_file_error() {
    let mut env = Env::new();
    let missing = path_str(&env.home.path().join("nonexistent-rival-tmp"));
    // The temp directory comes from TMPDIR on Unix and TMP first on Windows.
    let (var, sep) = if cfg!(windows) {
        ("TMP", '\\')
    } else {
        ("TMPDIR", '/')
    };
    let not_found = crate::errtext::NO_SUCH_PATH;
    env.set(var, Some(&missing));
    let err = build_claude_docker_image(&env.config()).unwrap_err();
    assert!(
        err.starts_with(&format!(
            "create temp dockerfile: open {missing}{sep}rival-claude-dockerfile-"
        )),
        "{err}"
    );
    assert!(err.ends_with(&format!(": {not_found}")), "{err}");
}

/// The volume mount source, as built on every OS: a workdir with a
/// leading "/" as given, anything else appended to the working directory
/// with a bare "/" and no cleaning. The quirk shows on Windows: a drive
/// path `C:\repo` is not "/"-rooted, so it mounts `<cwd>/C:\repo`.
#[test]
fn mount_source_keeps_absolute_and_joins_relative_workdirs() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("tok"));
    let cfg = env.config();
    let mount = |workdir: &str| {
        let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, workdir);
        let mut seen = None;
        run_claude_docker_with(
            &cfg,
            &mut sess,
            "p",
            "high",
            workdir,
            config::CLAUDE_MODEL,
            true,
            crate::executor::testutil::recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        let args = seen.unwrap().args;
        let i = args.iter().position(|a| a == "-v").unwrap();
        args[i + 1].clone()
    };
    let work = env.work_str();
    assert_eq!(mount("/repo"), "/repo:/workspace:ro");
    assert_eq!(mount("sub/../x"), format!("{work}/sub/../x:/workspace:ro"));
    assert_eq!(mount(r"C:\repo"), format!(r"{work}/C:\repo:/workspace:ro"));
}

/// The token never goes into the docker argv: other local users can read
/// process arguments. Docker gets the name only and copies the value from
/// the child environment.
#[test]
fn token_goes_by_name_only_and_value_in_the_child_env() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("s3cret-tok"));
    let cfg = env.config();
    let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, "/repo");
    let mut seen = None;
    run_claude_docker_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        "/repo",
        config::CLAUDE_MODEL,
        true,
        crate::executor::testutil::recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    let seen = seen.unwrap();
    assert!(
        !seen.args.iter().any(|a| a.contains("s3cret-tok")),
        "token in argv: {:?}",
        seen.args
    );
    let i = seen.args.iter().position(|a| a == "-e").unwrap();
    assert_eq!(seen.args[i + 1], "ANTHROPIC_AUTH_TOKEN");
    assert_eq!(
        seen.env,
        vec!["ANTHROPIC_AUTH_TOKEN=s3cret-tok".to_string()]
    );
}

/// The run names its container after the session, and removes it unless
/// docker exited 0 (when `--rm` already did). Killing the docker client on
/// a cancel or timeout leaves the container running against the mount.
#[cfg(unix)]
#[test]
fn failed_or_cancelled_runs_remove_their_container() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("tok"));
    let calls = fake_docker(&env, 0, 0, 0);
    let cfg = env.config();
    let run = |result: anyhow::Result<RunResult>| {
        let _ = std::fs::remove_file(&calls);
        let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, "/repo");
        let mut seen = None;
        let _ = run_claude_docker_with(
            &cfg,
            &mut sess,
            "p",
            "high",
            "/repo",
            config::CLAUDE_MODEL,
            true,
            crate::executor::testutil::recorder(&mut seen, result),
        );
        let args = seen.unwrap().args;
        let i = args.iter().position(|a| a == "--name").unwrap();
        assert_eq!(args[i + 1], container_name(&sess.id));
        let rm = std::fs::read_to_string(&calls).unwrap_or_default();
        (container_name(&sess.id), rm)
    };
    let (_, rm) = run(Ok(RunResult::default()));
    assert_eq!(rm, "", "exit 0: --rm already removed it");
    let (name, rm) = run(Ok(RunResult {
        exit_code: 137,
        ..RunResult::default()
    }));
    assert_eq!(rm, format!("rm -f {name}\n"));
    let (name, rm) = run(Err(anyhow::anyhow!("signal: killed")));
    assert_eq!(rm, format!("rm -f {name}\n"));
}

/// A hung Docker daemon must not hold the queue slot: the cleanup gives up
/// at its deadline and kills `docker rm`.
#[cfg(unix)]
#[test]
fn container_removal_gives_up_at_its_deadline() {
    let env = Env::new();
    env.fake("docker", "#!/bin/sh\nexec sleep 30\n");
    let started = std::time::Instant::now();
    remove_container(
        &env.config(),
        "rival-x",
        std::time::Duration::from_millis(300),
    );
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
}
