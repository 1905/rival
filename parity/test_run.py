"""Unit tests for the parity runner and fakes, against a purpose-built temp binary.

Run: python3 -m unittest discover -s parity -p 'test_*.py'
No real reviewer CLI, Go binary or network is involved: the "rival" here is a
small Python interpreter of JSON actions, and every path lives in a temp dir.
"""

import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock

import run  # puts parity/fakes on sys.path
import fakecli  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

# The binary under test: argv[1] is a JSON list of single-key actions.
FAKE_RIVAL = r'''#!{python} -IB
import faulthandler, json, os, shutil, signal, subprocess, sys, time, urllib.request

STACK_FILES = []  # faulthandler writes to these fds; keep them open while registered

def act(actions):
    for a in actions:
        (k, v), = a.items()
        if k == "out":
            sys.stdout.write(v); sys.stdout.flush()
        elif k == "err":
            sys.stderr.write(v); sys.stderr.flush()
        elif k == "log":
            sys.stderr.write(json.dumps(v) + "\n"); sys.stderr.flush()
        elif k == "call":
            exe = shutil.which(v["argv"][0])
            if exe is None:
                sys.stdout.write("missing %s\n" % v["argv"][0]); continue
            r = subprocess.run([exe] + v["argv"][1:], input=v.get("stdin", "").encode(),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=v.get("cwd"))
            sys.stdout.write("%s exit=%d out=%r err=%r\n" % (v["argv"][0], r.returncode,
                             r.stdout.decode(), r.stderr.decode()))
        elif k == "env":
            with open(v, "w") as f:
                json.dump(dict(os.environ), f)
        elif k == "write":
            os.makedirs(os.path.dirname(v[0]), exist_ok=True)
            with open(v[0], "w") as f:
                f.write(v[1])
        elif k == "wait_for":
            while not os.path.exists(v):
                time.sleep(0.02)
        elif k == "sleep":
            time.sleep(v)
        elif k in ("detach", "detach_inherit"):
            # detach_inherit keeps stdin too, as Go's detach does.
            child = subprocess.Popen([sys.argv[0], json.dumps(v)], start_new_session=True,
                                     stdin=None if k == "detach_inherit" else subprocess.DEVNULL)
            sys.stderr.write("rival: detached pid=%d\n" % child.pid); sys.stderr.flush()
        elif k == "stdin":
            sys.stdout.write(sys.stdin.read()); sys.stdout.flush()
        elif k == "exists":
            sys.stdout.write("exists=%s\n" % os.path.exists(v)); sys.stdout.flush()
        elif k == "pidfile":
            with open(v + ".tmp", "w") as f:
                f.write(str(os.getpid()))
            os.rename(v + ".tmp", v)
        elif k == "stack_ready":
            # Handshake: the <pid> marker appears only after SIGUSR1 is registered, so a
            # stack request cannot kill this pid. Each process dumps into its own
            # <pid>.stack file: fakes sharing the step's stderr would interleave.
            os.makedirs(v, exist_ok=True)
            STACK_FILES.append(open(os.path.join(v, "%d.stack" % os.getpid()), "w"))
            faulthandler.register(signal.SIGUSR1, file=STACK_FILES[-1], all_threads=True)
            open(os.path.join(v, str(os.getpid())), "w").close()
        elif k == "exec_clean":
            # Replaces this process with one that has no task token and no SIGUSR1 handler.
            # PARITY_TEST_OWNER is test-only: fallback cleanup finds it, the runner never reads it.
            os.execve(sys.executable, [sys.executable, "-c", "import time; time.sleep(%r)" % v],
                      {{"PARITY_TEST_OWNER": os.environ["FAKE_TASK_ROOT"]}})
        elif k == "http":
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({{}}))
            sys.stdout.write(opener.open(os.environ["RIVAL_UPDATE_API"] + v, timeout=5).read().decode())
        elif k == "exit":
            sys.exit(v)

act(json.loads(sys.argv[1]) if len(sys.argv) > 1 else [])
'''

TIME = "2026-10-02T09:00:00Z"


def step(actions, expect=None, **kw):
    s = dict(kw, run=[json.dumps(actions)])
    if expect is not None:
        s["expect"] = expect
    return s


def ok(stdout="", **kw):
    return dict({"exit_code": 0, "stdout": stdout}, **kw)


def scenario(steps, home_files=(), **kw):
    sc = {"name": kw.pop("name", "t"), "steps": steps, "expect": dict({"home_files": list(home_files)}, **kw.pop("expect", {}))}
    sc.update(kw)
    run.validate(sc)
    return sc


class RunnerCase(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="parity-test-")
        self.addCleanup(self.tmp.cleanup)
        self.bin = os.path.join(self.tmp.name, "fake-rival")
        with open(self.bin, "w") as f:
            f.write(FAKE_RIVAL.format(python=sys.executable))
        os.chmod(self.bin, 0o755)
        self.work = os.path.join(self.tmp.name, "runs")
        os.makedirs(self.work)
        # An injected parent environment; os.environ is never mutated.
        self.parent = {"USER": "tester", "OPENAI_API_KEY": "sk-leak", "ANTHROPIC_API_KEY": "leak",
                       "PATH": os.environ.get("PATH", ""), "HTTPS_PROXY": "http://proxy.invalid:1"}

    def run_sc(self, sc):
        return run.run_scenario(sc, self.bin, self.work, parent_env=self.parent)

    def assertPass(self, result):
        self.assertEqual((result.problems, result.unsafe), ([], []), "root: %s" % result.root)

    def assertProblem(self, result, pattern):
        joined = "\n".join(result.problems)
        self.assertRegex(joined, pattern)


class BasicTest(RunnerCase):
    def test_stdout_exit_and_plain_stderr(self):
        sc = scenario([step([{"out": "hi\n"}, {"err": "rival: detached pid=777\n"}, {"exit": 3}],
                            {"exit_code": 3, "stdout": "hi\n", "stderr_lines": ["rival: detached pid=<PID1>"]})])
        self.assertPass(self.run_sc(sc))

    def test_mismatches_are_reported(self):
        sc = scenario([step([{"out": "hi\n"}, {"err": "surprise\n"}, {"exit": 1}], ok("bye\n"))])
        r = self.run_sc(sc)
        self.assertProblem(r, "exit code 1, want 0")
        self.assertProblem(r, "stdout differs")
        self.assertProblem(r, "plain stderr lines differ")

    def test_uuid_prefix_links_steps_and_unknown_is_a_problem(self):
        u = "0b7c6a43-5d1e-4c2f-9a8b-1c2d3e4f5a6b"
        rules = [{"regex": r"(?m)^([0-9a-f]{8}) completed exit=", "kind": "uuid_prefix"}]
        log = {"level": "info", "app": "rival", "time": TIME, "message": "starting", "session": u}
        summary = step([{"out": u[:8] + " completed exit=0 1s\n"}], ok("<UUID1:8> completed exit=0 1s\n"))
        sc = scenario([step([{"log": log}], ok(log_events=[
            {"level": "info", "message": "starting", "fields": {"session": "<UUID1>"}}])), summary],
            normalise=rules)
        self.assertPass(self.run_sc(sc))
        r = self.run_sc(scenario([summary], normalise=rules))
        self.assertEqual(len(r.problems), 1, r.problems)
        self.assertProblem(r, r"^steps\[0\] .*: UUID prefix '0b7c6a43' matches 0 seen UUIDs, want 1$")

    def test_log_events(self):
        log_a = {"level": "info", "app": "rival", "session": "0b7c6a43-5d1e-4c2f-9a8b-1c2d3e4f5a6b",
                 "pid": 4321, "time": TIME, "message": "reaping orphaned session"}
        log_b = {"level": "warn", "app": "rival", "time": TIME, "message": "second"}
        expect = ok(log_events=[
            {"level": "warn", "message": "second"},
            {"level": "info", "message": "reaping orphaned session", "fields": {"session": "<UUID1>", "pid": "<PID1>"}},
        ])
        self.assertPass(self.run_sc(scenario([step([{"log": log_a}, {"log": log_b}], expect)])))
        # An unexpected extra event fails even when every expected one is seen.
        r = self.run_sc(scenario([step([{"log": log_a}, {"log": log_b}, {"log": log_b}], expect)]))
        self.assertProblem(r, "unexpected log event")
        # A JSON line without zerolog's shape fails.
        r = self.run_sc(scenario([step([{"log": {"level": "info", "message": "m"}}],
                                       ok(log_events=[{"level": "info", "message": "m"}]))]))
        self.assertProblem(r, "malformed log event")

    def test_home_files_and_file_checks(self):
        u = "0b7c6a43-5d1e-4c2f-9a8b-1c2d3e4f5a6b"
        session = {"id": u, "status": "completed", "pid": 99, "exit_code": 0, "output_bytes": 12,
                   "start_time": TIME, "work_dir": "<ROOT>/work"}
        sc = scenario(
            [step([{"write": ["<HOME>/.rival/sessions/%s.json" % u, json.dumps(session)]}], ok())],
            home_files=[".rival/config.yaml", ".rival/sessions/<UUID1>.json"],
            fixtures=[{"write": "home/.rival/config.yaml", "text": "efforts: {}\n"}],
            expect={"files": [
                {"path": "<HOME>/.rival/sessions/<UUID1>.json",
                 "json": {"id": "<UUID1>", "status": "completed", "pid": "<PID1>", "exit_code": 0,
                          "output_bytes": 12, "start_time": "<TIME>", "work_dir": "<ROOT>/work"}},
                {"path": "<HOME>/.rival/config.yaml", "text": "efforts: {}\n"},
                {"path": "<HOME>/.rival/queue/.lock", "absent": True},
            ]},
        )
        # Placeholders in argv reach the binary as real paths and normalise back.
        self.assertPass(self.run_sc(sc))
        sc["expect"]["files"][0]["json"]["output_bytes"] = 13
        self.assertProblem(self.run_sc(sc), "JSON differs")

    def test_schema_rejects_typos_and_missing_expectations(self):
        bad = [
            {"name": "x", "steps": [{"run": [], "expect": ok()}], "expect": {}},
            {"name": "x", "steps": [{"run": [], "expect": {"exit_code": 0}}], "expect": {"home_files": []}},
            {"name": "x", "steps": [{"run": [], "expect": ok(), "stdout": ""}], "expect": {"home_files": []}},
            {"name": "x", "steps": [{"run": [], "expect": ok(), "env": {"PATH": "/usr/bin"}}], "expect": {"home_files": []}},
            {"name": "x", "steps": [{"run": [], "expect": ok(), "env": {"FAKE_CODEX_SCRIPT": "/x"}}], "expect": {"home_files": []}},
            {"name": "x", "steps": [{"start": "a", "run": []}], "expect": {"home_files": []}},
            {"name": "x", "steps": [{"wait": "a", "expect": ok()}], "expect": {"home_files": []}},
            {"name": "x", "system_tools": ["codex"], "steps": [{"run": [], "expect": ok()}], "expect": {"home_files": []}},
            {"name": "x", "fixtures": [{"git": "r", "args": ["clone", "https://example.invalid/x"]}],
             "steps": [{"run": [], "expect": ok()}], "expect": {"home_files": []}},
            {"name": "x", "fakes": {"scripts": {"codex": {"responses": [{"argv": []}]}}},
             "steps": [{"run": [], "expect": ok()}], "expect": {"home_files": []}},
        ]
        for sc in bad:
            with self.subTest(sc=sc), self.assertRaises(run.ScenarioError):
                run.validate(sc)

    def test_schema_reports_bad_shapes_and_duplicates(self):
        def sc(steps=None, **kw):
            return dict({"name": "x", "steps": steps or [{"run": [], "expect": ok()}],
                         "expect": {"home_files": []}}, **kw)
        cases = {
            "env: want an object": sc(env=["A"]),
            r"steps\[0\]\.env: want an object": sc([{"run": [], "expect": ok(), "env": "A=1"}]),
            "fixtures: want a list": sc(fixtures={"write": "a"}),
            r"fixtures\[0\]: want an object": sc(fixtures=["a"]),
            "'git' is always linked": sc(system_tools=["git"]),
            "'ps' is listed twice": sc(system_tools=["ps", "ps"]),
            "update_server.routes: want an object": sc(update_server={"routes": []}),
            "expect.files: want a list": dict(sc(), expect={"home_files": [], "files": {}}),
            "at most one of stdin, stdin_file": sc([{"run": [], "expect": ok(), "stdin": "", "stdin_file": "a"}]),
            "unlink_stdin: needs stdin_file": sc([{"run": [], "expect": ok(), "unlink_stdin": True}]),
            "unlink_stdin: want true": sc([{"run": [], "expect": ok(), "stdin_file": "a", "unlink_stdin": False}]),
            r"stdout_file: want a string": sc([{"run": [], "expect": ok(), "stdout_file": 1}]),
            r"'\./o\.log' is already a named output": sc([{"run": [], "expect": ok(), "stdout_file": "o.log"},
                                                     {"run": [], "expect": ok(), "stderr_file": "./o.log"}]),
            "'same' is already a named output": sc([{"run": [], "expect": ok(), "stdout_file": "same",
                                                     "stderr_file": "same"}]),
        }
        for message, bad in cases.items():
            with self.subTest(message), self.assertRaisesRegex(run.ScenarioError, message):
                run.validate(bad)

    def test_invalid_scenario_file_is_a_report(self):
        sdir = os.path.join(self.tmp.name, "scenarios")
        os.makedirs(sdir)
        with open(os.path.join(sdir, "a.yaml"), "w") as f:
            json.dump({"name": "x", "env": ["A"], "steps": [{"run": [], "expect": ok()}],
                       "expect": {"home_files": []}}, f)
        out = subprocess.run([sys.executable, os.path.join(HERE, "run.py"), "--bin", self.bin,
                              "--scenarios-dir", sdir, "--work-dir", self.work],
                             env={"PATH": "/usr/bin:/bin"}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual((out.returncode, out.stderr), (1, b""))
        self.assertIn(b"FAIL a.yaml\n    env: want an object\n", out.stdout)

    def test_fixture_paths_stay_in_root(self):
        sc = scenario([step([], ok())], fixtures=[{"write": "../escape", "text": "x"}])
        self.assertProblem(self.run_sc(sc), "leaves the task root")

    def test_git_fixture_is_local(self):
        sc = scenario(
            [step([{"out": "ok\n"}], ok("ok\n"), cwd="work/repo")],
            fixtures=[{"write": "work/repo/a.txt", "text": "a\n"},
                      {"git": "work/repo", "args": ["init", "-q"]},
                      {"git": "work/repo", "args": ["add", "a.txt"]},
                      {"git": "work/repo", "args": ["commit", "-q", "-m", "init"]}])
        r = self.run_sc(sc)
        self.assertPass(r)
        # Unix seconds: Git versions spell a UTC iso-strict date as "Z" or "+00:00".
        head = subprocess.run(["git", "-C", os.path.join(r.root, "work/repo"), "log", "--format=%an %cn %at %ct"],
                              stdout=subprocess.PIPE, check=True).stdout.decode()
        self.assertEqual(head, "Rival Parity Rival Parity 1767225600 1767225600\n")  # 2026-01-01T00:00:00Z


class EnvTest(RunnerCase):
    def test_allowlisted_environment(self):
        sc = scenario([step([{"env": "<ROOT>/env.json"}], ok())], env={"CI": None, "EXTRA": "<HOME>/x"})
        r = self.run_sc(sc)
        self.assertPass(r)
        with open(os.path.join(r.root, "env.json")) as f:
            env = json.load(f)
        home = os.path.join(r.root, "home")
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertNotIn("HTTPS_PROXY", env)
        self.assertNotIn("CI", env)
        self.assertEqual(env["USER"], "tester")
        self.assertEqual(env["HOME"], home)
        self.assertEqual(env["USERPROFILE"], home)
        self.assertEqual(env["RIVAL_HOME"], os.path.join(home, ".rival"))
        self.assertEqual(env["RIVAL_NO_TELEMETRY"], "1")
        self.assertEqual(env["RIVAL_NO_UPDATE_CHECK"], "1")
        self.assertEqual(env["EXTRA"], home + "/x")
        self.assertRegex(env["RIVAL_UPDATE_API"], r"^http://127\.0\.0\.1:\d+$")
        self.assertEqual(env["PATH"], os.path.join(r.root, "bin") + os.pathsep + os.path.join(r.root, "sysbin"))
        self.assertEqual(sorted(os.listdir(os.path.join(r.root, "sysbin"))), ["git"])
        self.assertEqual(env["TMPDIR"], os.path.join(r.root, "tmp"))

    def test_absent_fake_fails_lookpath(self):
        sc = scenario([step([{"call": {"argv": ["docker", "info"]}}, {"call": {"argv": ["claude", "-p"]}}],
                            ok("missing docker\nclaude exit=97 out='' err=%r\n" % (
                                "fake claude: unscripted call (FAKE_CLAUDE_SCRIPT unset)\n",)))],
                      fakes={"absent": ["docker"]})
        r = self.run_sc(sc)
        self.assertFalse(os.path.exists(os.path.join(r.root, "bin", "docker")))
        # The unscripted claude call fails the scenario even though stdout matched.
        self.assertEqual(r.problems, ["fake claude: unscripted call (FAKE_CLAUDE_SCRIPT unset)",
                                      "fake claude called 1 times, want 0: [['-p']]"])


class FakeTest(RunnerCase):
    def codex_script(self):
        return {"responses": [
            {"argv": ["login", "status"], "times": 1, "stdout": "Logged in using ChatGPT\n", "exit": 0},
            {"argv_prefix": ["exec"], "times": 1, "stderr": "You've hit your usage limit.\n", "exit": 1,
             "expect": {"stdin": "review this", "cwd": "<ROOT>/work", "env": {"RIVAL_NO_TELEMETRY": "1", "CI": "1",
                                                                             "OPENAI_API_KEY": None}}},
            {"argv_prefix": ["exec"], "times": 1, "stdout": "{\"ok\":true}\n", "exit": 0,
             "expect": {"stdin": {"$regex": "second.*"}}},
        ]}

    def test_per_invocation_responses_and_counts(self):
        calls = [{"call": {"argv": ["codex", "login", "status"]}},
                 {"call": {"argv": ["codex", "exec", "--json", "-"], "stdin": "review this"}},
                 {"call": {"argv": ["codex", "exec", "--json", "-"], "stdin": "second try"}}]
        want = ("codex exit=0 out='Logged in using ChatGPT\\n' err=''\n"
                "codex exit=1 out='' err=\"You've hit your usage limit.\\n\"\n"
                "codex exit=0 out='{\"ok\":true}\\n' err=''\n")
        sc = scenario([step(calls, ok(want))], fakes={"scripts": {"codex": self.codex_script()}},
                      expect={"calls": {"codex": 3}})
        self.assertPass(self.run_sc(sc))

    def test_expectation_violation_and_unmatched_argv(self):
        calls = [{"call": {"argv": ["codex", "exec"], "stdin": "wrong"}},
                 {"call": {"argv": ["codex", "review"]}}]
        script = self.codex_script()
        script["responses"][0]["times"] = 1
        sc = scenario([step(calls, ok("codex exit=1 out='' err=\"You've hit your usage limit.\\n\"\n"
                                       "codex exit=97 out='' err=\"fake codex: no scripted response for argv ['review']\\n\"\n"))],
                      fakes={"scripts": {"codex": script}}, expect={"calls": {"codex": 2}})
        r = self.run_sc(sc)
        self.assertProblem(r, r"fake codex \['exec'\]: stdin 'wrong' does not match 'review this'")
        self.assertProblem(r, r"no scripted response for argv \['review'\]")
        self.assertProblem(r, r"responses\[0\] used 0 times, want 1")

    def test_fake_waits_and_signals(self):
        script = {"responses": [{"argv": ["run"], "touch": "<ROOT>/sync/started", "wait_for": "<ROOT>/sync/go",
                                 "stdout": "done\n", "exit": 0}]}
        sc = scenario([
            {"start": "a", "run": [json.dumps([{"call": {"argv": ["grok", "run"]}}])]},
            {"wait_file": "sync/started", "timeout": 10},
            step([{"out": "second\n"}], ok("second\n")),
            {"touch": "sync/go"},
            {"wait": "a", "expect": ok("grok exit=0 out='done\\n' err=''\n")},
        ], fixtures=[{"mkdir": "sync"}], fakes={"scripts": {"grok": script}}, expect={"calls": {"grok": 1}})
        self.assertPass(self.run_sc(sc))

    def test_fake_script_outside_root_is_refused(self):
        root = tempfile.mkdtemp(dir=self.tmp.name)
        os.makedirs(os.path.join(root, ".parity", "fakes"))
        outside = os.path.join(self.tmp.name, "codex.json")
        with open(outside, "w") as f:
            json.dump({"responses": [{"argv": [], "exit": 0}]}, f)
        r = self.call_fake("codex", [], root, {"FAKE_CODEX_SCRIPT": outside})
        self.assertEqual(r.returncode, fakecli.EXIT_UNSCRIPTED)
        with open(fakecli.calls_path(root, "codex")) as f:
            self.assertIn("outside the task root", json.loads(f.readline())["error"])

    def call_fake(self, name, argv, root, extra_env):
        lib = os.path.join(HERE, "fakes")
        code = "import sys; sys.path.insert(0, %r); import fakecli; sys.argv[0] = %r; sys.exit(fakecli.main(%r))" % (lib, name, name)
        env = dict({"PATH": "/usr/bin:/bin", "FAKE_TASK_ROOT": root}, **extra_env)
        return subprocess.run([sys.executable, "-IB", "-c", code] + argv, env=env, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)


class BrewGuardTest(RunnerCase):
    def brew(self, prefix):
        return {"responses": [
            {"argv": ["upgrade", "1905/tap/rival"], "exit": 0},
            {"argv": ["--prefix", "rival"], "stdout": prefix + "\n", "exit": 0},
        ]}

    def test_unsafe_prefix_stops_before_the_binary_runs(self):
        sc = scenario([step([{"write": ["<ROOT>/ran", "x"]}], ok())],
                      fakes={"scripts": {"brew": self.brew("/opt/homebrew/opt/rival")}})
        r = self.run_sc(sc)
        self.assertFalse(r.ok)
        self.assertRegex(r.unsafe[0], "outside the task root")
        self.assertFalse(os.path.exists(os.path.join(r.root, "ran")))

    def test_fake_brew_rejects_unsafe_prefix_at_runtime(self):
        root = tempfile.mkdtemp(dir=self.tmp.name)
        os.makedirs(os.path.join(root, ".parity", "fakes"))
        script = os.path.join(root, ".parity", "fakes", "brew.json")
        with open(script, "w") as f:
            json.dump(self.brew("/usr/local"), f)
        r = FakeTest.call_fake(self, "brew", ["--prefix", "rival"], root, {"FAKE_BREW_SCRIPT": script})
        self.assertEqual((r.returncode, r.stdout), (fakecli.EXIT_UNSAFE, b""))
        # The runner turns the fake's report into an unsafe result.
        task = run.Task(scenario([step([], ok())]), self.bin, root, {})
        task.check_calls()
        self.assertRegex(task.result.unsafe[0], "fake brew: brew prefix '/usr/local' is outside")
        main_out = self.main_with_unsafe()
        self.assertEqual(main_out, 3)

    def main_with_unsafe(self):
        sdir = os.path.join(self.tmp.name, "scenarios")
        os.makedirs(sdir)
        sc = scenario([step([], ok())], fakes={"scripts": {"brew": self.brew("/usr/local")}})
        with open(os.path.join(sdir, "a.yaml"), "w") as f:
            json.dump(sc, f)
        with open(os.devnull, "w") as null:
            stdout, sys.stdout = sys.stdout, null
            try:
                return run.main(["--bin", self.bin, "--scenarios-dir", sdir, "--work-dir", self.work])
            finally:
                sys.stdout = stdout

    def test_safe_prefix_with_copied_binary(self):
        prefix = "<ROOT>/brew/opt/rival"
        sc = scenario(
            [step([{"call": {"argv": ["brew", "upgrade", "1905/tap/rival"]}},
                   {"call": {"argv": ["brew", "--prefix", "rival"]}}],
                  ok("brew exit=0 out='' err=''\nbrew exit=0 out='<ROOT>/brew/opt/rival\\n' err=''\n"))],
            home_files=[], fixtures=[{"copy_bin": "brew/opt/rival/bin/rival"}],
            fakes={"scripts": {"brew": self.brew(prefix)}}, expect={"calls": {"brew": 2}})
        r = self.run_sc(sc)
        self.assertPass(r)
        self.assertTrue(os.access(os.path.join(r.root, "brew/opt/rival/bin/rival"), os.X_OK))

    def test_safe_prefix_needs_prepopulated_binary(self):
        sc = scenario([step([], ok())], fakes={"scripts": {"brew": self.brew("<ROOT>/brew/opt/rival")}})
        self.assertProblem(self.run_sc(sc), "add a copy_bin fixture")


class HttpTest(RunnerCase):
    def test_local_release_server(self):
        path = "/repos/1905/rival/releases/latest"
        sc = scenario([step([{"http": path}], ok('{"tag_name": "v9.9.9"}'))],
                      update_server={"routes": {path: {"status": 200, "json": {"tag_name": "v9.9.9"}}}},
                      expect={"http_requests": [{"method": "GET", "path": path}]})
        self.assertPass(self.run_sc(sc))

    def test_unexpected_request_fails(self):
        sc = scenario([step([{"http": "/elsewhere"}], ok())])
        r = self.run_sc(sc)
        self.assertProblem(r, "exit code 1")
        self.assertProblem(r, r"update server requests differ.*\n.*\n  actual:   \[\('GET', '/elsewhere'\)\]")

    def test_server_start_does_no_name_lookup(self):
        # Stock HTTPServer.server_bind calls socket.getfqdn(), a reverse DNS query.
        with unittest.mock.patch("socket.getfqdn", side_effect=AssertionError("getfqdn called")):
            server = run.UpdateServer({})
        self.addCleanup(server.close)
        self.assertEqual(server.httpd.server_name, "127.0.0.1")
        self.assertRegex(server.url, r"^http://127\.0\.0\.1:\d+$")


class RedirectTest(RunnerCase):
    """Task 2.2's case: arbitrary redirect names, stdin unlinked once the detached parent exits."""

    def redirect_scenario(self, want_stdout):
        reader = [{"wait_for": "<ROOT>/sync/go"}, {"exists": "<ROOT>/in/prompt.txt"}, {"stdin": True},
                  {"err": "child read\n"}]
        return scenario([
            step([{"detach_inherit": reader}],
                 ok(want_stdout, stderr_lines=["rival: detached pid=<PID1>", "child read"]),
                 stdin_file="in/prompt.txt", unlink_stdin=True,
                 stdout_file="out/review result.log", stderr_file="logs/errors.txt"),
            {"touch": "sync/go"},
            {"wait_idle": True, "timeout": 10},
        ], fixtures=[{"write": "in/prompt.txt", "text": "review this diff\n"}, {"mkdir": "sync"}],
            expect={"files": [{"path": "<ROOT>/in/prompt.txt", "absent": True},
                              {"path": "<ROOT>/logs/errors.txt", "text": "rival: detached pid=%s\nchild read\n"
                               % "<PID1>"}]})

    def test_detached_reader_survives_unlinked_stdin(self):
        r = self.run_sc(self.redirect_scenario("exists=False\nreview this diff\n"))
        self.assertPass(r)
        with open(os.path.join(r.root, "out", "review result.log")) as f:
            self.assertEqual(f.read(), "exists=False\nreview this diff\n")
        self.assertEqual(sorted(os.listdir(os.path.join(r.root, ".parity", "steps"))), [])

    def test_named_stdout_is_still_checked_exactly(self):
        r = self.run_sc(self.redirect_scenario("exists=False\nsomething else\n"))
        self.assertEqual(len(r.problems), 1, r.problems)
        self.assertProblem(r, r"stdout differs\n(.|\n)*'review this diff\\n'")

    def test_redirect_paths_must_be_task_owned(self):
        cases = {
            r"stdin_file '\.parity/scenario\.json' is inside the runner's \.parity dir": {"stdin_file": ".parity/scenario.json"},
            r"stdin_file 'in/missing' is not an existing regular file": {"stdin_file": "in/missing"},
            r"stdin_file 'in/link' is not an existing regular file": {"stdin_file": "in/link"},
            r"stdout_file 'in/prompt.txt' already exists": {"stdout_file": "in/prompt.txt"},
            r"path '\.\./outside' leaves the task root": {"stderr_file": "../outside"},
        }
        for message, redirect in cases.items():
            with self.subTest(message):
                sc = scenario([step([{"write": ["<ROOT>/ran", "x"]}], ok(), **redirect)],
                              fixtures=[{"write": "in/prompt.txt", "text": "x\n"}])
                task = run.Task(sc, self.bin, tempfile.mkdtemp(dir=self.work), self.parent)
                os.makedirs(os.path.join(task.root, "in"))
                os.symlink("prompt.txt", os.path.join(task.root, "in", "link"))
                r = task.run()
                self.assertEqual(len(r.problems), 1, r.problems)
                self.assertRegex(r.problems[0], "^ScenarioError: " + message)
                self.assertFalse(os.path.exists(os.path.join(r.root, "ran")))
                self.assertTrue(os.path.exists(os.path.join(r.root, "in", "prompt.txt")))


class SemanticFileTest(RunnerCase):
    """Concurrent reviewers: first-seen UUID order is a race, so sessions bind by their fields."""

    CODEX = "11111111-1111-4111-8111-111111111111"
    CLAUDE = "22222222-2222-4222-8222-222222222222"

    GROUP = "33333333-3333-4333-8333-333333333333"

    def raced(self, first, second, codex_event_id=None, group=None):
        """Writes both sessions and logs both start events in the given order."""
        actions = []
        for cli, sid in (first, second):
            log = {"level": "info", "app": "rival", "time": TIME, "message": "starting plan reviewer",
                   "session": codex_event_id if (cli == "codex" and codex_event_id) else sid, "reviewer": cli}
            actions.append({"log": log})
        for cli, sid in (first, second):
            body = {"id": sid, "cli": cli, "model": cli + "-model", "status": "completed",
                    "group": group if (group and cli == "claude") else self.GROUP}
            actions.append({"write": ["<HOME>/.rival/sessions/%s.json" % sid, json.dumps(body)]})
        return actions

    def sc(self, actions):
        events = [{"level": "info", "message": "starting plan reviewer",
                   "fields": {"session": "<ID:%s>" % cli.upper(), "reviewer": cli}} for cli in ("codex", "claude")]
        sessions = "<HOME>/.rival/sessions/*.json"
        return scenario(
            [step(actions, ok(log_events=events))],
            home_files=[".rival/sessions/<ID:CODEX>.json", ".rival/sessions/<ID:CLAUDE>.json"],
            expect={"files": [
                {"glob": sessions, "where": {"cli": "codex"}, "bind": {"CODEX": "id", "GROUP": "group"},
                 "json": {"id": "<ID:CODEX>", "model": "codex-model", "status": "completed"}},
                {"glob": sessions, "where": {"cli": "claude"}, "bind": {"CLAUDE": "id", "GROUP": "group"},
                 "json": {"id": "<ID:CLAUDE>", "model": "claude-model", "status": "completed"}},
            ]})

    def test_binding_passes_in_either_arrival_order(self):
        codex, claude = ("codex", self.CODEX), ("claude", self.CLAUDE)
        self.assertPass(self.run_sc(self.sc(self.raced(codex, claude))))
        self.assertPass(self.run_sc(self.sc(self.raced(claude, codex))))

    def test_swapped_association_fails(self):
        # The codex start event names the claude session.
        r = self.run_sc(self.sc(self.raced(("codex", self.CODEX), ("claude", self.CLAUDE),
                                           codex_event_id=self.CLAUDE)))
        self.assertProblem(r, "expected log event not seen")

    def test_wrong_model_in_bound_file_fails(self):
        sc = self.sc(self.raced(("codex", self.CODEX), ("claude", self.CLAUDE)))
        sc["expect"]["files"][0]["json"]["model"] = "claude-model"
        self.assertProblem(self.run_sc(sc), r"selected by .* JSON differs")

    def test_shared_binding_must_agree(self):
        other = "44444444-4444-4444-8444-444444444444"
        r = self.run_sc(self.sc(self.raced(("codex", self.CODEX), ("claude", self.CLAUDE), group=other)))
        self.assertProblem(r, "differs from the UUID already bound to GROUP")

    def test_where_must_select_exactly_one_file(self):
        sc = self.sc(self.raced(("codex", self.CODEX), ("claude", self.CLAUDE)))
        sc["expect"]["files"][0]["where"] = {"status": "completed"}
        self.assertProblem(self.run_sc(sc), r"files\[0\] glob .*: 2 files match, want 1")
        sc["expect"]["files"][0]["where"] = {"cli": "grok"}
        self.assertProblem(self.run_sc(sc), r"files\[0\] glob .*: 0 files match, want 1")

    def test_schema(self):
        base = {"name": "x", "steps": [step([], ok())]}
        g = {"glob": "<HOME>/*.json", "where": {"cli": "codex"}, "json": {}}
        bad = [
            {"home_files": ["<ID:NOPE>"]},
            {"home_files": [], "files": [dict(g, where={})]},
            {"home_files": [], "files": [dict(g, path="<HOME>/x")]},
            {"home_files": [], "files": [{"glob": "<HOME>/*", "where": {"a": 1}, "absent": True}]},
            {"home_files": [], "files": [dict(g, bind={"lower": "id"})]},
            {"home_files": [], "files": [dict(g, bind={"A": ""})]},
            {"home_files": [], "files": [dict(g, bind=["A"])]},
            {"home_files": [], "files": [{"path": "<HOME>/x", "where": {"a": 1}, "json": {}}]},
            {"home_files": [], "dirs": [{"path": "<ROOT>/tmp"}]},
            {"home_files": [], "dirs": [{"path": "<ROOT>/tmp", "entries": "x"}]},
        ]
        for expect in bad:
            with self.assertRaises(run.ScenarioError, msg=expect):
                run.validate(dict(base, expect=expect))
        run.validate(dict(base, expect={"home_files": ["<ID:A>"], "files": [dict(g, bind={"A": "id"})]}))


class StdoutRegexTest(RunnerCase):
    def test_structural_stdout(self):
        rx = {"$regex": "(?s)(?=.*\\bqueue\\b)(?!.*forbidden).*"}
        self.assertPass(self.run_sc(scenario([step([{"out": "a\nqueue\nb\n"}], ok(rx))])))
        r = self.run_sc(scenario([step([{"out": "queue forbidden\n"}], ok(rx))]))
        self.assertProblem(r, "stdout does not match")
        with self.assertRaises(run.ScenarioError):
            scenario([step([], ok({"$regex": "("}))])
        with self.assertRaises(run.ScenarioError):
            scenario([step([], ok({"$regex": "x", "extra": 1}))])


class Sha256Test(RunnerCase):
    def test_raw_bytes_are_hashed_without_normalisation(self):
        import hashlib
        text = '{"checked_at":"%s"}' % TIME
        digest = hashlib.sha256(text.encode()).hexdigest()
        sc = scenario([step([], ok())], home_files=["kept.json"],
                      fixtures=[{"write": "home/kept.json", "text": text}],
                      expect={"files": [{"path": "<HOME>/kept.json", "sha256": digest}]})
        self.assertPass(self.run_sc(sc))
        # A rewritten timestamp normalises to the same <TIME> text, but not the same bytes.
        sc["steps"] = [step([{"write": ["<HOME>/kept.json", '{"checked_at":"2026-10-02T09:00:01Z"}']}], ok())]
        self.assertProblem(self.run_sc(sc), r"file <HOME>/kept.json sha256 [0-9a-f]{64}, want " + digest)

    def test_schema(self):
        base = {"name": "x", "steps": [step([], ok())]}
        for f in ({"path": "<HOME>/a", "sha256": "ABC"}, {"path": "<HOME>/a", "sha256": "0" * 64, "text": ""}):
            with self.assertRaises(run.ScenarioError, msg=f):
                run.validate(dict(base, expect={"home_files": [], "files": [f]}))


class DirTest(RunnerCase):
    def test_empty_dir_passes_and_leftovers_fail(self):
        sc = scenario([step([{"write": ["<ROOT>/tmp/keep/x", "1"]}], ok())],
                      expect={"dirs": [{"path": "<ROOT>/tmp", "entries": ["keep"]},
                                       {"path": "<ROOT>/work", "entries": []}]})
        self.assertPass(self.run_sc(sc))
        sc["expect"]["dirs"][0]["entries"] = []
        self.assertProblem(self.run_sc(sc), r"dir <ROOT>/tmp entries differ\n.*expected: \[\]\n.*actual:   \['keep'\]")

    def test_missing_dir_is_not_an_empty_dir(self):
        sc = scenario([step([], ok())], expect={"dirs": [{"path": "<ROOT>/tmp/gone", "entries": []}]})
        self.assertProblem(self.run_sc(sc), r"dir <ROOT>/tmp/gone missing")


class ProcessTest(RunnerCase):
    def test_late_detached_output_is_checked(self):
        # The child writes only after the parent's exit was checked; it is gone before the sweep.
        def sc(lines):
            return scenario([
                step([{"detach": [{"wait_for": "<ROOT>/sync/go"}, {"err": "late line\n"}]}],
                     ok(stderr_lines=lines)),
                {"touch": "sync/go"},
                {"wait_idle": True, "timeout": 10},
            ], fixtures=[{"mkdir": "sync"}])
        r = self.run_sc(sc(["rival: detached pid=<PID1>"]))
        self.assertEqual(len(r.problems), 1, r.problems)
        self.assertProblem(r, r"plain stderr lines differ\n.*\n  actual:   \['rival: detached pid=<PID1>', 'late line'\]")
        self.assertPass(self.run_sc(sc(["rival: detached pid=<PID1>", "late line"])))

    def test_exit_code_is_checked_per_step(self):
        sc = scenario([step([{"exit": 2}], ok()), step([{"exit": 0}], ok())])
        r = self.run_sc(sc)
        self.assertEqual(r.problems, ["steps[0] %s: exit code 2, want 0" % [json.dumps([{"exit": 2}])]])

    def test_signals_are_deferred_inside_critical_sections(self):
        sig = run.Signals()
        with self.assertRaises(run.Interrupted) as caught:
            with sig.deferred():
                sig.handler(signal.SIGTERM, None)
                self.assertEqual(sig.pending, signal.SIGTERM)
        self.assertEqual((caught.exception.signum, caught.exception.name), (signal.SIGTERM, "SIGTERM"))
        # Cleanup after the first signal cannot be interrupted again.
        sig.handler(signal.SIGINT, None)
        with self.assertRaises(run.Interrupted):
            run.Signals().handler(signal.SIGINT, None)

    def test_signal_cleans_task_processes_and_spares_others(self):
        for signum in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signal.Signals(signum).name):
                self.check_signal_cleanup(signum)

    def start_runner(self, label, actions, bin_path=None):
        """run.py as a subprocess on a one-step scenario. Returns (runner, work dir, unrelated sleeper)."""
        sdir = os.path.join(self.tmp.name, "scenarios-" + label)
        work = os.path.join(self.tmp.name, "work-" + label)
        os.makedirs(sdir)
        with open(os.path.join(sdir, "a.yaml"), "w") as f:
            json.dump(scenario([step(actions, ok(), timeout=120)]), f)
        other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                 env={"PATH": "/usr/bin:/bin"})
        self.addCleanup(other.communicate)
        self.addCleanup(other.kill)
        runner = subprocess.Popen([sys.executable, os.path.join(HERE, "run.py"), "--bin", bin_path or self.bin,
                                   "--scenarios-dir", sdir, "--work-dir", work],
                                  env={"PATH": "/usr/bin:/bin"}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return runner, work, other

    def check_signal_cleanup(self, signum):
        name = signal.Signals(signum).name
        # The step's own process drops the token (exec_clean): only tracking can find it.
        runner, work, other = self.start_runner(name, [
            {"stack_ready": "<ROOT>/ready"},
            {"detach": [{"stack_ready": "<ROOT>/ready"}, {"pidfile": "<ROOT>/child.pid"}, {"sleep": 60}]},
            {"wait_for": "<ROOT>/child.pid"}, {"pidfile": "<ROOT>/parent.pid"}, {"exec_clean": 60}])
        try:
            pids = self.wait_pids(work, runner)
            self.assertTrue(all(_alive(p) for p in pids))
            runner.send_signal(signum)
            out, err = runner.communicate(timeout=20)
            self.assertEqual((runner.returncode, err), (128 + signum, b""), out)
            self.assertIn(("interrupted by %s; task processes cleaned up" % name).encode(), out)
            self.assertIn(("interrupted by %s: run stopped" % name).encode(), out)
            deadline = time.monotonic() + 5
            while any(_alive(p) for p in pids) and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertEqual([p for p in pids if _alive(p)], [])
            self.assertIsNone(other.poll())
        finally:
            if runner.returncode is None:
                _stop_runner(runner)
            _kill_task_leftovers(work)

    def wait_pids(self, work, runner, bound=10):
        """PIDs of the step's process and its detached child, once both pidfiles exist.

        A runner exit or a stall fails with bounded evidence: which pidfiles arrived,
        the runner's output and stacks, and every step's stdout/stderr.
        """
        started = time.monotonic()
        while time.monotonic() - started < bound and runner.poll() is None:
            files = [os.path.join(r, n) for r in _roots(work) for n in ("parent.pid", "child.pid")]
            if files and all(os.path.exists(f) for f in files):
                # parent.pid is written just before exec_clean; give the exec a moment.
                time.sleep(0.3)
                pids = []
                for f in files:
                    with open(f) as fh:
                        pids.append(int(fh.read()))
                return pids
            time.sleep(0.05)
        self.fail(_readiness_report(work, runner, time.monotonic() - started))

    def test_readiness_failure_reports_stall_and_cleans_up(self):
        # parent.pid never arrives: the report must show the stacks and stop the task.
        # plain.pid has no stack_ready handshake, so it must not get SIGUSR1.
        ready = "<ROOT>/ready"
        runner, work, other = self.start_runner("stall", [
            {"stack_ready": ready},
            {"detach": [{"stack_ready": ready}, {"pidfile": "<ROOT>/child.pid"}, {"sleep": 60}]},
            {"detach": [{"pidfile": "<ROOT>/plain.pid"}, {"sleep": 60}]},
            {"pidfile": "<ROOT>/stalled.pid"}, {"wait_for": "<ROOT>/never"}])
        names = ("child.pid", "plain.pid", "stalled.pid")
        try:
            started = time.monotonic()
            while not any(all(os.path.exists(os.path.join(r, n)) for n in names) for r in _roots(work)):
                if time.monotonic() - started > 10 or runner.poll() is not None:
                    self.fail(_readiness_report(work, runner, time.monotonic() - started))
                time.sleep(0.05)
            (root,) = _roots(work)
            pids = {}
            for n in names:
                with open(os.path.join(root, n)) as f:
                    pids[n] = int(f.read())
            child = pids["child.pid"]
            with self.assertRaises(AssertionError) as caught:
                self.wait_pids(work, runner, bound=0.2)
        finally:
            if runner.returncode is None:
                _stop_runner(runner)
            _kill_task_leftovers(work)
        report = str(caught.exception)
        self.assertIn("runner still running; stack dumps requested with SIGUSR1 from runner %d and "
                      "registered fakes %s" % (runner.pid, sorted([child, pids["stalled.pid"]])), report)
        self.assertIn("runner stopped by SIGTERM or its own exit", report)
        self.assertIn("interrupted by SIGTERM: run stopped", report)
        self.assertIn("root %s: pidfiles ['child.pid']" % root, report)
        runner_err = _report_section(report, "runner stderr:")
        self.assertIn("most recent call first", runner_err)
        self.assertIn("in run_steps", runner_err)
        # Fakes dump into their own files: two dumps into one stderr interleaved on CI.
        step_err = _report_section(report, "step file 01.stderr:")
        self.assertIn("rival: detached pid=%d" % child, step_err)
        self.assertNotIn("most recent call first", step_err)
        for pid in (child, pids["stalled.pid"]):
            with self.subTest(fake=pid):
                stack = _report_section(report, "fake %d stack file:" % pid)
                self.assertIn("most recent call first", stack)
                self.assertIn("in act", stack)
        self.assertEqual([p for p in pids.values() if _alive(p)], [])
        self.assertIsNone(other.poll())

    def test_readiness_failure_skips_sigusr1_before_registration(self):
        # No scenario root yet: the runner may not have registered SIGUSR1, whose default kills it.
        runner = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                  env={"PATH": "/usr/bin:/bin"}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: runner.poll() is None and runner.kill())
        work = os.path.join(self.tmp.name, "work-unregistered")
        os.makedirs(work)
        report = _readiness_report(work, runner, 0.0)
        self.assertIn("runner still running before its first scenario root; "
                      "SIGUSR1 not sent (handler not proven registered)", report)
        self.assertEqual(runner.returncode, -signal.SIGTERM)

    def test_leftover_cleanup_matches_owner_marker_only(self):
        # Fallback after a forced runner kill: the token-less exec_clean process goes, others stay.
        work = os.path.join(self.tmp.name, "work-leftovers")
        os.makedirs(os.path.join(work, "root"))
        (root,) = _roots(work)
        sleep = [sys.executable, "-c", "import time; time.sleep(60)"]
        owned = subprocess.Popen(sleep, env={"PARITY_TEST_OWNER": root})
        foreign = subprocess.Popen(sleep, env={"PARITY_TEST_OWNER": root + "-other"})
        bare = subprocess.Popen(sleep, env={"PATH": "/usr/bin:/bin"})
        for p in (owned, foreign, bare):
            self.addCleanup(p.wait)
            self.addCleanup(lambda p=p: p.poll() is None and p.kill())
        deadline = time.monotonic() + 5
        while owned.pid not in run.pids_with_env("PARITY_TEST_OWNER", root) and time.monotonic() < deadline:
            time.sleep(0.05)
        _kill_task_leftovers(work)
        self.assertEqual(owned.wait(5), -signal.SIGKILL)
        self.assertEqual((foreign.poll(), bare.poll()), (None, None))

    def test_readiness_failure_reports_early_runner_exit(self):
        runner, work, _ = self.start_runner("exit", [], bin_path=os.path.join(self.tmp.name, "missing"))
        with self.assertRaises(AssertionError) as caught:
            self.wait_pids(work, runner)
        report = str(caught.exception)
        self.assertIn("runner exited early with code 2", report)
        self.assertIn("is not an executable file", report)
        self.assertIn("no scenario root under the work dir", report)

    def test_detached_child_shares_redirect_and_wait_idle(self):
        sc = scenario([
            {"start": "a", "run": [json.dumps([{"detach": [{"sleep": 0.3}, {"err": "child done\n"}]}, {"out": "parent\n"}])]},
            {"wait_idle": True, "timeout": 10},
            {"wait": "a", "expect": ok("parent\n", stderr_lines=["rival: detached pid=<PID1>", "child done"])},
        ])
        self.assertPass(self.run_sc(sc))

    def test_leaked_process_is_killed_and_fails(self):
        sc = scenario([step([{"detach": [{"sleep": 60}]}], ok(stderr_lines=["rival: detached pid=<PID1>"]))])
        started = time.monotonic()
        r = self.run_sc(sc)
        self.assertProblem(r, "leaked task processes")
        self.assertLess(time.monotonic() - started, 15)
        token = re.search(r"leaked task processes \[(\d+)", "\n".join(r.problems))
        self.assertIsNotNone(token)
        pid = int(token.group(1))
        time.sleep(0.2)
        self.assertFalse(_alive(pid))

    def test_step_timeout_kills_the_process_group(self):
        sc = scenario([step([{"sleep": 30}], ok(), timeout=0.5)])
        started = time.monotonic()
        r = self.run_sc(sc)
        self.assertLess(time.monotonic() - started, 10)
        self.assertProblem(r, "timed out; process group killed")

    def test_owned_pids_only_sees_the_token(self):
        token = "unit-%d" % os.getpid()
        env = {"PATH": "/usr/bin:/bin", run.TOKEN_VAR: token}
        mine = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], env=env)
        other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], env={"PATH": "/usr/bin:/bin"})
        try:
            deadline = time.monotonic() + 5
            while mine.pid not in run.owned_pids(token) and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertEqual(run.owned_pids(token), [mine.pid])
            self.assertEqual(run.kill_owned(token), [mine.pid])
            mine.wait(5)
            self.assertIsNone(other.poll())
        finally:
            for p in (mine, other):
                if p.poll() is None:
                    p.kill()
                p.wait()


def _alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def _roots(work):
    """Scenario roots under a runner's --work-dir, as the runner's realpaths."""
    if not os.path.isdir(work):
        return []
    return [os.path.realpath(os.path.join(work, n)) for n in sorted(os.listdir(work))]


def _tail(data, limit=4000):
    text = data.decode("utf-8", "replace")
    return text if len(text) <= limit else "[%d chars cut]...%s" % (len(text) - limit, text[-limit:])


def _signal_pids(pids, signum):
    for pid in pids:
        try:
            os.kill(pid, signum)
        except ProcessLookupError:
            pass


def _stop_runner(runner, grace=20):
    """SIGTERM, so the runner cleans its own task processes; SIGKILL after grace.

    Returns (stdout, stderr, forced).
    """
    if runner.poll() is None:
        runner.send_signal(signal.SIGTERM)
    try:
        out, err = runner.communicate(timeout=grace)
        return out, err, False
    except subprocess.TimeoutExpired:
        runner.kill()
        out, err = runner.communicate()
        return out, err, True


def _kill_task_leftovers(work):
    """SIGKILLs processes whose environment names a task root under work.

    FAKE_TASK_ROOT tags every fake-rival process; PARITY_TEST_OWNER tags the exec_clean
    process, which has no runner token. Unrelated processes carry neither root.
    """
    for root in _roots(work):
        _signal_pids(run.pids_with_env("FAKE_TASK_ROOT", root) + run.pids_with_env("PARITY_TEST_OWNER", root),
                     signal.SIGKILL)


def _stack_ready(root):
    """Fake PIDs that finished the stack_ready handshake and still carry this root.

    The environment check drops a PID that exec'd (exec resets the handler) or was reused.
    """
    ready = os.path.join(root, "ready")
    marked = {int(n) for n in os.listdir(ready) if n.isdigit()} if os.path.isdir(ready) else set()
    return sorted(marked & set(run.pids_with_env("FAKE_TASK_ROOT", root)))


def _readiness_report(work, runner, elapsed):
    """Bounded evidence for a run that never got ready; also stops the run and its processes."""
    lines = ["scenario processes not ready under %s after %.1fs" % (work, elapsed)]
    fakes = {}  # root -> handshaked fake PIDs that were asked for stacks
    if runner.poll() is not None:
        lines.append("runner exited early with code %s" % runner.returncode)
    elif not _roots(work):
        # SIGUSR1's default action kills; only a scenario root proves main() registered faulthandler.
        lines.append("runner still running before its first scenario root; "
                     "SIGUSR1 not sent (handler not proven registered)")
    else:
        # The runner dumps into its stderr pipe; each handshaked fake into its own stack file.
        fakes = {r: _stack_ready(r) for r in _roots(work)}
        pids = sorted(p for ps in fakes.values() for p in ps)
        lines.append("runner still running; stack dumps requested with SIGUSR1 from runner %d and "
                     "registered fakes %s" % (runner.pid, pids))
        _signal_pids([runner.pid] + pids, signal.SIGUSR1)
        stacks = [_stack_file(r, p) for r, ps in fakes.items() for p in ps]
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not all(os.path.getsize(s) for s in stacks):
            time.sleep(0.05)
        time.sleep(0.5)  # the runner's own dump goes to a pipe that cannot be polled here
    out, err, forced = _stop_runner(runner)
    _kill_task_leftovers(work)
    lines.append("runner stopped by %s" % ("SIGKILL after 20s" if forced else "SIGTERM or its own exit"))
    lines += ["runner stdout:\n" + _tail(out), "runner stderr:\n" + _tail(err)]
    if not _roots(work):
        lines.append("no scenario root under the work dir")
    for root in _roots(work):
        arrived = [n for n in ("child.pid", "parent.pid") if os.path.exists(os.path.join(root, n))]
        lines.append("root %s: pidfiles %s; entries %s" % (root, arrived, sorted(os.listdir(root))))
        steps = os.path.join(root, ".parity", "steps")
        for name in sorted(os.listdir(steps)) if os.path.isdir(steps) else ():
            with open(os.path.join(steps, name), "rb") as f:
                lines.append("step file %s:\n%s" % (name, _tail(f.read())))
        for pid in fakes.get(root, ()):
            with open(_stack_file(root, pid), "rb") as f:
                lines.append("fake %d stack file:\n%s" % (pid, _tail(f.read())))
    return "\n".join(lines)


def _stack_file(root, pid):
    """Where a fake that passed the stack_ready handshake writes its SIGUSR1 stack dump."""
    return os.path.join(root, "ready", "%d.stack" % pid)


def _report_section(report, header):
    """The report text after header, up to the next report header line."""
    text = report.split(header + "\n", 1)[1]
    return re.split(r"\n(?=runner |root |step file |fake \d+ stack file:)", text, maxsplit=1)[0]


class ScenarioFilesTest(unittest.TestCase):
    def test_all_scenarios_validate(self):
        # Later tasks add scenarios: the initial three must stay, and every file must validate.
        names = [n for n in sorted(os.listdir(run.SCENARIOS_DIR)) if n.endswith(".yaml")]
        self.assertLessEqual({"queue-empty.yaml", "sessions-empty.yaml", "version.yaml"}, set(names))
        for n in names:
            with self.subTest(n):
                run.load_scenario(os.path.join(run.SCENARIOS_DIR, n))

    # Committed expectations, independent of the removed Go tree.
    # Source (Go, before removal): rival/cmd/root.go `const banner` and `var Version = "dev"`, printed by
    # rival/cmd/version.go as fmt.Print(banner) then fmt.Printf("  %s\\n", Version).
    GO_BANNER = ("\n"
                 "         _             __\n"
                 "   _____(_)   ______ _/ /\n"
                 "  / ___/ / | / / __ `/ /\n"
                 " / /  / /| |/ / /_/ / /\n"
                 "/_/  /_/ |___/\\__,_/_/\n")
    EXPECTED_STDOUT = {
        "version": GO_BANNER + "  dev\n",
        # rival/cmd/sessions.go sessionsAction: fmt.Println("No sessions found.")
        "sessions-empty": "No sessions found.\n",
        # rival/cmd/queue.go queueListAction: fmt.Println("Queue is empty.")
        "queue-empty": "Queue is empty.\n",
    }

    def test_initial_scenarios_match_committed_go_output(self):
        for name, stdout in self.EXPECTED_STDOUT.items():
            with self.subTest(name):
                sc = run.load_scenario(os.path.join(run.SCENARIOS_DIR, name + ".yaml"))
                self.assertEqual(len(sc["steps"]), 1)
                self.assertEqual(sc["steps"][0]["expect"], {"exit_code": 0, "stdout": stdout,
                                                            "stderr_lines": [], "log_events": []})


if __name__ == "__main__":
    unittest.main()
