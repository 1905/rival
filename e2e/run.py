#!/usr/bin/env python3
"""Rival e2e scenario runner (Rust binary only). See e2e/README.md.

Usage: e2e/run.py --bin <rival> [--scenario <glob>]... [--work-dir <dir>]

Exit codes: 0 all scenarios passed, 1 a scenario failed, 2 bad arguments,
3 a safety guard fired (unsafe brew prefix); the run stops at that point.
SIGTERM/SIGINT clean up the running scenario's processes, then exit 128+signal.
"""

from __future__ import annotations

import argparse
import contextlib
import difflib
import faulthandler
import fnmatch
import glob
import hashlib
import http.server
import json
import os
import re
import shutil
import signal
import socketserver
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
FAKES_DIR = os.path.join(HERE, "fakes")
SCENARIOS_DIR = os.path.join(HERE, "scenarios")
sys.path.insert(0, HERE)
sys.path.insert(0, FAKES_DIR)

import fakecli  # noqa: E402
import normalise  # noqa: E402

TOKEN_VAR = "RIVAL_E2E_TASK"
INHERITED_VARS = ("USER", "LOGNAME")
FIXED_ENV = {
    "TZ": "UTC",
    "CI": "1",
    "RIVAL_NO_TELEMETRY": "1",
    "RIVAL_NO_UPDATE_CHECK": "1",
    "GIT_CONFIG_NOSYSTEM": "1",
}
PROTECTED_VARS = frozenset({
    "PATH", "HOME", "USERPROFILE", "RIVAL_HOME", "TMPDIR", "RIVAL_NO_TELEMETRY",
    "RIVAL_UPDATE_API", "GIT_CONFIG_NOSYSTEM", TOKEN_VAR,
})
SYSTEM_SEARCH = "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin"
DEFAULT_SYSTEM_TOOLS = ("git",)
FORBIDDEN_GIT = frozenset({"clone", "fetch", "pull", "push", "ls-remote", "submodule", "archive", "bundle"})
GIT_IDENTITY = {
    # Fixed identity: scenarios pin commit hashes, which depend on it.
    "GIT_AUTHOR_NAME": "Rival Parity",
    "GIT_AUTHOR_EMAIL": "parity@example.invalid",
    "GIT_COMMITTER_NAME": "Rival Parity",
    "GIT_COMMITTER_EMAIL": "parity@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00+00:00",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00+00:00",
}
RUN_KEYS = ("run", "cwd", "stdin", "stdin_file", "unlink_stdin", "stdout_file", "stderr_file", "env", "timeout")
DEFAULT_STEP_TIMEOUT = 30.0
BIND_RE = re.compile(r"<ID:([A-Z0-9_]+)>")
DEFAULT_SCENARIO_TIMEOUT = 300.0

LAUNCHER = """#!{python} -IB
import sys
sys.path.insert(0, {lib!r})
import fakecli
sys.exit(fakecli.main({name!r}))
"""


class ScenarioError(Exception):
    """The scenario file or its setup is invalid."""


# ---------------------------------------------------------------- schema


def _keys(obj, where, allowed, required=()):
    if not isinstance(obj, dict):
        raise ScenarioError("%s: want an object" % where)
    unknown = set(obj) - set(allowed)
    if unknown:
        raise ScenarioError("%s: unknown keys %s" % (where, sorted(unknown)))
    missing = set(required) - set(obj)
    if missing:
        raise ScenarioError("%s: missing keys %s" % (where, sorted(missing)))


def _str_list(value, where):
    if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
        raise ScenarioError("%s: want a list of strings" % where)


def _list(value, where):
    if not isinstance(value, list):
        raise ScenarioError("%s: want a list" % where)
    return value


def _timeout(step, where):
    t = step.get("timeout", DEFAULT_STEP_TIMEOUT)
    if type(t) not in (int, float) or t <= 0:
        raise ScenarioError("%s.timeout: want a positive number" % where)


def _env(env, where):
    if not isinstance(env, dict):
        raise ScenarioError("%s: want an object" % where)
    for key, value in env.items():
        if key in PROTECTED_VARS or key.startswith("FAKE_"):
            raise ScenarioError("%s: %s is set by the runner" % (where, key))
        if value is not None and not isinstance(value, str):
            raise ScenarioError("%s.%s: want a string or null (unset)" % (where, key))


def _redirects(step, where, outputs):
    """Named redirects: paths are checked against the root when the step starts."""
    for key in ("stdin", "stdin_file", "stdout_file", "stderr_file"):
        if key in step and not isinstance(step[key], str):
            raise ScenarioError("%s.%s: want a string" % (where, key))
    if "stdin" in step and "stdin_file" in step:
        raise ScenarioError(where + ": set at most one of stdin, stdin_file")
    if "unlink_stdin" in step:
        if step["unlink_stdin"] is not True:
            raise ScenarioError(where + ".unlink_stdin: want true")
        if "stdin_file" not in step:
            raise ScenarioError(where + ".unlink_stdin: needs stdin_file")
    for key in ("stdout_file", "stderr_file"):
        if key not in step:
            continue
        name = os.path.normpath(step[key])
        if name in outputs:
            raise ScenarioError("%s.%s: %r is already a named output; each must be new" % (where, key, step[key]))
        outputs.add(name)


def _step_expect(expect, where):
    _keys(expect, where, ("exit_code", "stdout", "stderr_lines", "log_events"), ("exit_code", "stdout"))
    if type(expect["exit_code"]) is not int:
        raise ScenarioError(where + ".exit_code: want an integer")
    out = expect["stdout"]
    if not isinstance(out, str) and not (isinstance(out, dict) and set(out) == {"$regex"}
                                         and isinstance(out["$regex"], str)):
        raise ScenarioError(where + ".stdout: want a string or {\"$regex\": ...}")
    if isinstance(out, dict):
        try:
            re.compile(out["$regex"])
        except re.error as err:
            raise ScenarioError("%s.stdout: bad $regex: %s" % (where, err))
    _str_list(expect.get("stderr_lines", []), where + ".stderr_lines")
    problems = normalise.validate_expected_events(expect.get("log_events", []))
    if problems:
        raise ScenarioError("%s: %s" % (where, "; ".join(problems)))


def validate(sc):
    """Rejects unknown keys and missing expectations instead of skipping a check."""
    _keys(sc, "scenario", ("name", "description", "timeout", "fakes", "system_tools", "env", "fixtures",
                           "update_server", "normalise", "steps", "expect"), ("name", "steps", "expect"))
    if not isinstance(sc["name"], str) or not sc["name"]:
        raise ScenarioError("name: want a non-empty string")
    fakes = sc.get("fakes", {})
    _keys(fakes, "fakes", ("absent", "scripts"))
    _str_list(fakes.get("absent", []), "fakes.absent")
    scripts = fakes.get("scripts", {})
    _keys(scripts, "fakes.scripts", fakecli.NAMES)
    for name in fakes.get("absent", []):
        if name not in fakecli.NAMES:
            raise ScenarioError("fakes.absent: unknown fake %r" % name)
        if name in scripts:
            raise ScenarioError("fakes: %s is both absent and scripted" % name)
    for name, script in scripts.items():
        problems = fakecli.validate_script(script)
        if problems:
            raise ScenarioError("fakes.scripts.%s: %s" % (name, "; ".join(problems)))
    _str_list(sc.get("system_tools", []), "system_tools")
    for i, tool in enumerate(sc.get("system_tools", [])):
        if tool in fakecli.NAMES or os.sep in tool:
            raise ScenarioError("system_tools: %r may not be linked" % tool)
        if tool in DEFAULT_SYSTEM_TOOLS:
            raise ScenarioError("system_tools: %r is always linked; do not list it" % tool)
        if tool in sc["system_tools"][:i]:
            raise ScenarioError("system_tools: %r is listed twice" % tool)
    _env(sc.get("env", {}), "env")
    for i, fx in enumerate(_list(sc.get("fixtures", []), "fixtures")):
        where = "fixtures[%d]" % i
        if not isinstance(fx, dict):
            raise ScenarioError(where + ": want an object")
        if "write" in fx:
            _keys(fx, where, ("write", "text", "json", "mode"), ("write",))
            if ("text" in fx) == ("json" in fx):
                raise ScenarioError(where + ": set exactly one of text, json")
            if "mode" in fx and not re.fullmatch(r"0[0-7]{3}", str(fx["mode"])):
                raise ScenarioError(where + ".mode: want an octal string like \"0600\"")
        elif "mkdir" in fx:
            _keys(fx, where, ("mkdir",))
        elif "copy_bin" in fx:
            _keys(fx, where, ("copy_bin",))
        elif "git" in fx:
            _keys(fx, where, ("git", "args"), ("git", "args"))
            _str_list(fx["args"], where + ".args")
            if not fx["args"] or fx["args"][0] in FORBIDDEN_GIT:
                raise ScenarioError(where + ".args: local git commands only")
        else:
            raise ScenarioError(where + ": want write, mkdir, copy_bin or git")
    server = sc.get("update_server", {})
    _keys(server, "update_server", ("routes",))
    routes = server.get("routes", {})
    if not isinstance(routes, dict):
        raise ScenarioError("update_server.routes: want an object")
    for path, route in routes.items():
        where = "update_server.routes[%r]" % path
        _keys(route, where, ("status", "json", "body", "content_type"), ("status",))
        if ("json" in route) == ("body" in route):
            raise ScenarioError(where + ": set exactly one of json, body")
    problems = normalise.validate_rules(sc.get("normalise", []))
    if problems:
        raise ScenarioError("; ".join(problems))
    if "timeout" in sc:
        _timeout(sc, "scenario")
    steps = sc["steps"]
    if not isinstance(steps, list) or not steps:
        raise ScenarioError("steps: want a non-empty list")
    handles = set()
    outputs = set()
    for i, step in enumerate(steps):
        where = "steps[%d]" % i
        if not isinstance(step, dict):
            raise ScenarioError(where + ": want an object")
        if "run" in step:
            if "start" in step:
                _keys(step, where, ("start",) + RUN_KEYS)
                if step["start"] in handles:
                    raise ScenarioError("%s: handle %r already started" % (where, step["start"]))
                handles.add(step["start"])
            else:
                _keys(step, where, RUN_KEYS + ("expect",), ("expect",))
                _step_expect(step["expect"], where + ".expect")
            _str_list(step["run"], where + ".run")
            _env(step.get("env", {}), where + ".env")
            _timeout(step, where)
            _redirects(step, where, outputs)
        elif "wait" in step:
            _keys(step, where, ("wait", "timeout", "expect"), ("expect",))
            if step["wait"] not in handles:
                raise ScenarioError("%s: handle %r was not started" % (where, step["wait"]))
            handles.discard(step["wait"])
            _step_expect(step["expect"], where + ".expect")
            _timeout(step, where)
        elif "wait_file" in step:
            _keys(step, where, ("wait_file", "timeout"))
            _timeout(step, where)
        elif "touch" in step:
            _keys(step, where, ("touch",))
        elif "wait_idle" in step:
            _keys(step, where, ("wait_idle", "timeout"))
            if step["wait_idle"] is not True:
                raise ScenarioError(where + ".wait_idle: want true")
            _timeout(step, where)
        else:
            raise ScenarioError(where + ": want run, wait, wait_file, touch or wait_idle")
    if handles:
        raise ScenarioError("handles never waited: %s" % sorted(handles))
    expect = sc["expect"]
    _keys(expect, "expect", ("home_files", "files", "dirs", "calls", "http_requests"), ("home_files",))
    _str_list(expect["home_files"], "expect.home_files")
    bound = set()
    for i, f in enumerate(_list(expect.get("files", []), "expect.files")):
        where = "expect.files[%d]" % i
        _keys(f, where, ("path", "glob", "where", "bind", "json", "text", "sha256", "absent"))
        if ("path" in f) == ("glob" in f):
            raise ScenarioError(where + ": set exactly one of path, glob")
        if sum(k in f for k in ("json", "text", "sha256", "absent")) != 1:
            raise ScenarioError(where + ": set exactly one of json, text, sha256, absent")
        if "sha256" in f and not (isinstance(f["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", f["sha256"])):
            raise ScenarioError(where + ".sha256: want 64 lowercase hex digits")
        if "absent" in f and f["absent"] is not True:
            raise ScenarioError(where + ".absent: want true")
        if "glob" in f:
            if not isinstance(f["glob"], str) or not isinstance(f.get("where"), dict) or not f["where"]:
                raise ScenarioError(where + ": glob needs a string glob and a non-empty where object")
            if "absent" in f:
                raise ScenarioError(where + ": a glob selects one existing file; absent is not allowed")
            bind = f.get("bind", {})
            if not isinstance(bind, dict) or not all(
                    re.fullmatch(r"[A-Z0-9_]+", k) and isinstance(v, str) and v for k, v in bind.items()):
                raise ScenarioError(where + ".bind: want {NAME: json_field} with NAME of [A-Z0-9_]")
            bound.update(bind)
        elif "where" in f or "bind" in f:
            raise ScenarioError(where + ": where and bind need glob")
    for i, d in enumerate(_list(expect.get("dirs", []), "expect.dirs")):
        where = "expect.dirs[%d]" % i
        _keys(d, where, ("path", "entries"), ("path", "entries"))
        if not isinstance(d["path"], str):
            raise ScenarioError(where + ".path: want a string")
        _str_list(d["entries"], where + ".entries")
    used = set(BIND_RE.findall(json.dumps([steps, expect])))
    if used - bound:
        raise ScenarioError("<ID:...> names never bound by a files glob: %s" % sorted(used - bound))
    calls = expect.get("calls", {})
    _keys(calls, "expect.calls", fakecli.NAMES)
    for name, n in calls.items():
        if type(n) is not int or n < 0:
            raise ScenarioError("expect.calls.%s: want an integer >= 0" % name)
    for i, req in enumerate(_list(expect.get("http_requests", []), "expect.http_requests")):
        _keys(req, "expect.http_requests[%d]" % i, ("method", "path"), ("method", "path"))


def load_scenario(path):
    """Scenarios are YAML files restricted to its JSON subset (stdlib json only)."""
    with open(path, encoding="utf-8") as f:
        try:
            sc = json.load(f)
        except ValueError as err:
            raise ScenarioError("%s: not JSON-subset YAML: %s" % (path, err))
    validate(sc)
    return sc


# ---------------------------------------------------------------- processes


class Interrupted(BaseException):
    """SIGTERM or SIGINT reached the runner; the running scenario cleans up and the run stops."""

    def __init__(self, signum):
        super().__init__(signum)
        self.signum = signum

    @property
    def name(self):
        return signal.Signals(self.signum).name


class Signals:
    """Turns the first SIGTERM/SIGINT into Interrupted.

    Inside deferred() the signal waits until the block ends, so a spawned process
    is always tracked and cleanup always completes. Later signals are ignored
    while that cleanup runs.
    """

    def __init__(self):
        self.depth = 0
        self.pending = None
        self.fired = None

    def install(self):
        """Returns the previous handlers for restore()."""
        return {sig: signal.signal(sig, self.handler) for sig in (signal.SIGTERM, signal.SIGINT)}

    @staticmethod
    def restore(previous):
        for sig, handler in previous.items():
            signal.signal(sig, handler)

    def handler(self, signum, frame):
        if self.fired is not None:
            return
        if self.depth:
            self.pending = signum
            return
        self.fired = signum
        raise Interrupted(signum)

    @contextlib.contextmanager
    def deferred(self):
        self.depth += 1
        try:
            yield
        finally:
            self.depth -= 1
            if not self.depth and self.pending is not None and self.fired is None:
                self.fired, self.pending = self.pending, None
                raise Interrupted(self.fired)


SIGNALS = Signals()


def owned_pids(token):
    """PIDs of live processes whose environment carries this task's token."""
    return pids_with_env(TOKEN_VAR, token)


def pids_with_env(var, value):
    """PIDs of live processes (not this one) whose environment has var=value; value has no spaces."""
    needle = "%s=%s" % (var, value)
    me = os.getpid()
    found = []
    if sys.platform.startswith("linux"):
        for entry in os.listdir("/proc"):
            if not entry.isdigit() or int(entry) == me:
                continue
            try:
                with open("/proc/%s/environ" % entry, "rb") as f:
                    items = f.read().split(b"\0")
            except OSError:
                continue
            if needle.encode() in items:
                found.append(int(entry))
    elif sys.platform == "darwin":
        out = subprocess.run(["/bin/ps", "-axwwE", "-o", "pid=,command="], stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, env={"PATH": "/usr/bin:/bin"}, check=False).stdout
        pattern = re.compile(r"(?:^|\s)%s(?:\s|$)" % re.escape(needle))
        for line in out.decode("utf-8", "replace").splitlines():
            pid, _, rest = line.strip().partition(" ")
            if pid.isdigit() and int(pid) != me and pattern.search(rest):
                found.append(int(pid))
    else:
        raise RuntimeError("process sweep supports Linux and macOS only")
    return found


def kill_owned(token, grace=1.0):
    """TERM, then KILL, every process carrying the token. Returns the PIDs signalled."""
    killed = []
    for sig in (signal.SIGTERM, signal.SIGKILL):
        pids = owned_pids(token)
        if not pids:
            break
        for pid in pids:
            try:
                os.kill(pid, sig)
                killed.append(pid)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline and owned_pids(token):
            time.sleep(0.05)
    return sorted(set(killed))


class Handle:
    def __init__(self, label, popen, out_path, err_path, unlink=None):
        self.label = label
        self.popen = popen
        self.out_path = out_path
        self.err_path = err_path
        self.unlink = unlink
        self.timed_out = False

    def wait(self, timeout):
        try:
            self.popen.wait(timeout=max(timeout, 0))
        except subprocess.TimeoutExpired:
            self.timed_out = True
            self.kill()
            self.popen.wait()

    def kill(self):
        # An unreaped leader keeps its pid, so the group id cannot be reused yet.
        if self.popen.returncode is not None:
            return
        try:
            os.killpg(self.popen.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass


class UpdateServer:
    """Canned release API on 127.0.0.1; every request is recorded."""

    def __init__(self, routes):
        self.routes = routes
        self.requests = []
        self.lock = threading.Lock()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def _serve(self):
                length = int(self.headers.get("Content-Length") or 0)
                if length:
                    self.rfile.read(length)
                with owner.lock:
                    owner.requests.append({"method": self.command, "path": self.path})
                route = owner.routes.get(self.path)
                if route is None:
                    status, body, ctype = 404, b'{"message":"Not Found"}', "application/json"
                elif "json" in route:
                    status, ctype = route["status"], route.get("content_type", "application/json")
                    body = json.dumps(route["json"]).encode()
                else:
                    status, ctype = route["status"], route.get("content_type", "text/plain")
                    body = route["body"].encode()
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)

            do_GET = do_POST = do_HEAD = _serve

            def log_message(self, *args):
                pass

        class Server(http.server.ThreadingHTTPServer):
            def server_bind(self):
                # HTTPServer.server_bind calls socket.getfqdn(): a reverse DNS lookup,
                # which may leave the machine. The handler never uses the name.
                socketserver.TCPServer.server_bind(self)
                self.server_name, self.server_port = "127.0.0.1", self.server_address[1]

        self.httpd = Server(("127.0.0.1", 0), Handler)
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return "http://127.0.0.1:%d" % self.httpd.server_address[1]

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()


# ---------------------------------------------------------------- task


class Result:
    def __init__(self, name, root):
        self.name = name
        self.root = root
        self.problems = []
        self.unsafe = []
        self.interrupted = None

    @property
    def ok(self):
        return not self.problems and not self.unsafe and self.interrupted is None


def _diff(want, got):
    lines = difflib.unified_diff([repr(x) + "\n" for x in want.splitlines(True)],
                                 [repr(x) + "\n" for x in got.splitlines(True)], "expected", "actual")
    return "".join(lines)


def _read(path):
    with open(path, "rb") as f:
        return f.read().decode("utf-8", "replace")


class Task:
    """One scenario in its own temp root: HOME, fakes, fixtures, server, steps, checks."""

    def __init__(self, sc, bin_path, root, parent_env, python=sys.executable):
        self.sc = sc
        self.bin = bin_path
        self.root = os.path.realpath(root)
        self.home = os.path.join(self.root, "home")
        self.e2e = os.path.join(self.root, ".e2e")
        self.bin_dir = os.path.join(self.root, "bin")
        self.sys_dir = os.path.join(self.root, "sysbin")
        self.parent_env = parent_env
        self.python = python
        self.token = uuid.uuid4().hex
        self.result = Result(sc["name"], self.root)
        self.absent = set(sc.get("fakes", {}).get("absent", []))
        self.scripts = {}
        self.handles = {}
        self.live = []
        self.outputs = []
        self.server = None
        self.server_closed = False
        self.seq = 0
        self.bindings = {}
        self.selected = {}
        self.norm = normalise.Normaliser(
            [(self.home, "<HOME>"), (root, "<ROOT>"), (self.root, "<ROOT>"),
             (bin_path, "<BIN>"), (os.path.realpath(bin_path), "<BIN>")],
            sc.get("normalise", []))

    def problem(self, msg):
        self.result.problems.append(msg)

    # -- inputs

    def subst(self, value, regex=False):
        if isinstance(value, str):
            for token, path in (("<HOME>", self.home), ("<ROOT>", self.root), ("<BIN>", self.bin)):
                value = value.replace(token, re.escape(path) if regex else path)
            return value
        if isinstance(value, dict):
            if set(value) == {"$regex"}:
                return {"$regex": self.subst(value["$regex"], True)}
            return {k: self.subst(v, regex) for k, v in value.items()}
        if isinstance(value, list):
            return [self.subst(v, regex) for v in value]
        return value

    def path(self, rel):
        p = self.subst(rel)
        p = os.path.normpath(p if os.path.isabs(p) else os.path.join(self.root, p))
        if not fakecli.inside(self.root, p):
            raise ScenarioError("path %r leaves the task root" % rel)
        return p

    def owned_path(self, rel, key):
        """A path inside the root that is not the runner's own .e2e state."""
        p = self.path(rel)
        if p == self.e2e or fakecli.inside(self.e2e, p):
            raise ScenarioError("%s %r is inside the runner's .e2e dir" % (key, rel))
        return p

    def env(self, step_env=None):
        env = {k: self.parent_env[k] for k in INHERITED_VARS if k in self.parent_env}
        env.update(FIXED_ENV)
        env.update({
            "PATH": self.bin_dir + os.pathsep + self.sys_dir,
            "HOME": self.home,
            "USERPROFILE": self.home,
            "RIVAL_HOME": os.path.join(self.home, ".rival"),
            "TMPDIR": os.path.join(self.root, "tmp"),
            "RIVAL_UPDATE_API": self.server.url,
            "FAKE_TASK_ROOT": self.root,
            TOKEN_VAR: self.token,
        })
        for name in self.scripts:
            env[fakecli.script_env(name)] = os.path.join(self.e2e, "fakes", name + ".json")
        for layer in (self.sc.get("env", {}), step_env or {}):
            for key, value in layer.items():
                if value is None:
                    env.pop(key, None)
                else:
                    env[key] = self.subst(value)
        return env

    # -- setup

    def setup(self):
        for d in (self.home, self.bin_dir, self.sys_dir, os.path.join(self.root, "tmp"),
                  os.path.join(self.root, "work"), os.path.join(self.e2e, "fakes"),
                  os.path.join(self.e2e, "steps"), os.path.join(self.e2e, "fakelib")):
            os.makedirs(d, exist_ok=True)
        with open(os.path.join(self.e2e, "scenario.json"), "w", encoding="utf-8") as f:
            json.dump(self.sc, f, indent=2)
        self._install_fakes()
        self._link_system_tools()
        for fx in self.sc.get("fixtures", []):
            self._fixture(fx)
        self._check_brew()

    def _install_fakes(self):
        if any(c.isspace() for c in self.python):
            raise ScenarioError("python path %r has whitespace; shebang cannot hold it" % self.python)
        lib = os.path.join(self.e2e, "fakelib")
        shutil.copyfile(os.path.join(FAKES_DIR, "fakecli.py"), os.path.join(lib, "fakecli.py"))
        for name in fakecli.NAMES:
            if name in self.absent:
                continue
            launcher = os.path.join(self.bin_dir, name)
            with open(launcher, "w", encoding="utf-8") as f:
                f.write(LAUNCHER.format(python=self.python, lib=lib, name=name))
            os.chmod(launcher, 0o755)
        for name, script in self.sc.get("fakes", {}).get("scripts", {}).items():
            script = self.subst(script)
            for i, r in enumerate(script["responses"]):
                for key in ("wait_for", "touch"):
                    if key in r and not fakecli.inside(self.root, r[key]):
                        raise ScenarioError("fakes.scripts.%s.responses[%d].%s leaves the task root" % (name, i, key))
            self.scripts[name] = script
            with open(os.path.join(self.e2e, "fakes", name + ".json"), "w", encoding="utf-8") as f:
                json.dump(script, f, indent=2)

    def _link_system_tools(self):
        for tool in DEFAULT_SYSTEM_TOOLS + tuple(self.sc.get("system_tools", [])):
            real = shutil.which(tool, path=SYSTEM_SEARCH)
            if real is None:
                raise ScenarioError("system tool %r not found in %s" % (tool, SYSTEM_SEARCH))
            os.symlink(real, os.path.join(self.sys_dir, tool))
        path = self.bin_dir + os.pathsep + self.sys_dir
        for name in fakecli.NAMES:
            want = None if name in self.absent else os.path.join(self.bin_dir, name)
            if shutil.which(name, path=path) != want:
                raise ScenarioError("PATH lookup of %s gives %r, want %r" % (name, shutil.which(name, path=path), want))

    def _fixture(self, fx):
        if "write" in fx:
            p = self.path(fx["write"])
            os.makedirs(os.path.dirname(p), exist_ok=True)
            data = self.subst(fx["text"]) if "text" in fx else json.dumps(self.subst(fx["json"]), indent=2) + "\n"
            with open(p, "w", encoding="utf-8") as f:
                f.write(data)
            if "mode" in fx:
                os.chmod(p, int(fx["mode"], 8))
        elif "mkdir" in fx:
            os.makedirs(self.path(fx["mkdir"]), exist_ok=True)
        elif "copy_bin" in fx:
            p = self.path(fx["copy_bin"])
            os.makedirs(os.path.dirname(p), exist_ok=True)
            shutil.copyfile(self.bin, p)
            os.chmod(p, 0o755)
        else:
            repo = self.path(fx["git"])
            os.makedirs(repo, exist_ok=True)
            env = dict(GIT_IDENTITY, PATH="/usr/bin:/bin", HOME=self.home, TZ="UTC",
                       GIT_CONFIG_NOSYSTEM="1", GIT_TERMINAL_PROMPT="0")
            git = os.path.join(self.sys_dir, "git")
            cmd = [git, "-c", "protocol.allow=never", "-c", "init.defaultBranch=main",
                   "-c", "commit.gpgsign=false", "-C", repo] + self.subst(fx["args"])
            done = subprocess.run(cmd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, timeout=30, check=False)
            if done.returncode != 0:
                raise ScenarioError("git %s failed: %s" % (fx["args"], done.stdout.decode("utf-8", "replace")))

    def _check_brew(self):
        """Runs before any rival process: a non-temp prefix would let `rival update` exec it."""
        if "brew" not in self.scripts:
            return
        for i, r in enumerate(self.scripts["brew"]["responses"]):
            if not fakecli.argv_matches(r, ["--prefix", "rival"]):
                continue
            unsafe = fakecli.prefix_problem(self.root, r.get("stdout", ""))
            if unsafe:
                self.result.unsafe.append("brew responses[%d]: %s" % (i, unsafe))
                continue
            binary = os.path.join(r["stdout"].strip(), "bin", "rival")
            if not os.path.isfile(binary):
                raise ScenarioError("brew prefix has no bin/rival; add a copy_bin fixture for %s" % binary)

    # -- steps

    def output_path(self, step, key, default):
        if key not in step:
            return default
        p = self.owned_path(step[key], key)
        if os.path.lexists(p):
            raise ScenarioError("%s %r already exists; named outputs must be new" % (key, step[key]))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        return p

    def start(self, step, label):
        self.seq += 1
        base = os.path.join(self.e2e, "steps", "%02d" % self.seq)
        out_path = self.output_path(step, "stdout_file", base + ".stdout")
        err_path = self.output_path(step, "stderr_file", base + ".stderr")
        stdin_path, unlink = None, None
        if "stdin_file" in step:
            stdin_path = self.owned_path(step["stdin_file"], "stdin_file")
            try:
                regular = stat.S_ISREG(os.lstat(stdin_path).st_mode)
            except FileNotFoundError:
                regular = False
            if not regular:
                raise ScenarioError("stdin_file %r is not an existing regular file" % step["stdin_file"])
            if step.get("unlink_stdin"):
                unlink = stdin_path
        elif "stdin" in step:
            stdin_path = base + ".stdin"
            with open(stdin_path, "w", encoding="utf-8") as f:
                f.write(self.subst(step["stdin"]))
        cwd, env = self.path(step.get("cwd", "work")), self.env(step.get("env"))
        with contextlib.ExitStack() as files:
            stdin = files.enter_context(open(stdin_path, "rb")) if stdin_path else subprocess.DEVNULL
            out = files.enter_context(open(out_path, "wb"))
            err = files.enter_context(open(err_path, "wb"))
            # Spawn and tracking happen as one unit: an interrupt cannot land between them.
            with SIGNALS.deferred():
                popen = subprocess.Popen([self.bin] + self.subst(step["run"]), cwd=cwd, env=env, stdin=stdin,
                                         stdout=out, stderr=err, start_new_session=True, close_fds=True)
                handle = Handle(label, popen, out_path, err_path, unlink)
                self.live.append(handle)
        return handle

    def finish(self, handle, expect):
        """Checks what the parent decided; its output waits until every task writer is gone."""
        if handle.unlink is not None:
            # The step's own input, unlinked once the parent exits: a detached
            # child must keep reading through its inherited descriptor.
            os.unlink(handle.unlink)
            handle.unlink = None
        label = handle.label
        if handle.timed_out:
            self.problem("%s: timed out; process group killed" % label)
        code = handle.popen.returncode
        if code != expect["exit_code"]:
            self.problem("%s: exit code %s, want %s" % (label, code, expect["exit_code"]))
        self.outputs.append((handle, expect))

    def check_output(self, handle, expect):
        label = handle.label
        stdout = self.norm.text(_read(handle.out_path))
        plain, events = normalise.split_stderr(_read(handle.err_path))
        plain = [self.norm.text(line) for line in plain]
        normalised = [self.norm.json(e) for e in events]
        # Bound names resolve only now: the actual output assigned its markers first.
        expect = self.resolve(expect)
        if isinstance(expect["stdout"], dict):
            # Structural check (clap help and completion scripts): a full match.
            if not normalise.matches(expect["stdout"], stdout):
                self.problem("%s: stdout does not match %r\n  actual: %r" % (label, expect["stdout"]["$regex"], stdout))
        elif stdout != expect["stdout"]:
            self.problem("%s: stdout differs\n%s" % (label, _diff(expect["stdout"], stdout)))
        if plain != expect.get("stderr_lines", []):
            self.problem("%s: plain stderr lines differ\n  expected: %r\n  actual:   %r"
                         % (label, expect.get("stderr_lines", []), plain))
        for event in events:
            bad = normalise.event_problems(event)
            if bad:
                self.problem("%s: malformed log event %s: %s" % (label, json.dumps(event), ", ".join(bad)))
        missing, extra = normalise.match_events(expect.get("log_events", []), normalised)
        for e in missing:
            self.problem("%s: expected log event not seen: %s" % (label, json.dumps(e, sort_keys=True)))
        for e in extra:
            self.problem("%s: unexpected log event: %s" % (label, json.dumps(e, sort_keys=True)))

    def deadline_for(self, step):
        return min(time.monotonic() + step.get("timeout", DEFAULT_STEP_TIMEOUT), self.deadline)

    def run_steps(self):
        for i, step in enumerate(self.sc["steps"]):
            label = "steps[%d]" % i
            if "run" in step:
                handle = self.start(step, "%s %s" % (label, step["run"]))
                if "start" in step:
                    self.handles[step["start"]] = (handle, self.deadline_for(step))
                    continue
                handle.wait(self.deadline_for(step) - time.monotonic())
                self.finish(handle, step["expect"])
            elif "wait" in step:
                handle, started_deadline = self.handles.pop(step["wait"])
                handle.label = "%s wait %s" % (label, step["wait"])
                handle.wait(min(started_deadline, self.deadline_for(step)) - time.monotonic())
                self.finish(handle, step["expect"])
            elif "wait_file" in step:
                p, deadline = self.path(step["wait_file"]), self.deadline_for(step)
                while not os.path.exists(p):
                    if time.monotonic() > deadline:
                        self.problem("%s: timed out waiting for %s" % (label, step["wait_file"]))
                        return
                    time.sleep(0.02)
            elif "touch" in step:
                p = self.path(step["touch"])
                os.makedirs(os.path.dirname(p), exist_ok=True)
                open(p, "a").close()
            else:
                # Tracked handles are polled directly too: they count even
                # when their environment no longer carries the token.
                deadline = self.deadline_for(step)
                while any(h.popen.poll() is None for h in self.live) or owned_pids(self.token):
                    if time.monotonic() > deadline:
                        self.problem("%s: task processes still running: %s" % (label, owned_pids(self.token)))
                        return
                    time.sleep(0.1)

    # -- final checks

    def resolve(self, value):
        """Replaces <ID:NAME> with the UUID marker of the file bound to NAME."""
        if isinstance(value, str):
            def sub(m):
                raw = self.bindings.get(m.group(1))
                return m.group(0) if raw is None else self.norm.marker("UUID", raw)
            return BIND_RE.sub(sub, value)
        if isinstance(value, dict):
            return {k: self.resolve(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self.resolve(v) for v in value]
        return value

    def bind_files(self):
        """Selects each glob entry's one file by raw JSON fields, before any output check."""
        for i, f in enumerate(self.sc["expect"].get("files", [])):
            if "glob" not in f:
                continue
            pattern = self.subst(f["glob"])
            found = []
            for dirpath, dirnames, filenames in os.walk(self.root):
                dirnames[:] = sorted(d for d in dirnames if os.path.join(dirpath, d) != self.e2e)
                for name in sorted(filenames):
                    p = os.path.join(dirpath, name)
                    if not fnmatch.fnmatchcase(p, pattern):
                        continue
                    try:
                        data = json.loads(_read(p))
                    except ValueError:
                        continue
                    if normalise.matches(f["where"], data):
                        found.append((p, data))
            label = "files[%d] glob %s where %s" % (i, f["glob"], json.dumps(f["where"], sort_keys=True))
            if len(found) != 1:
                self.problem("%s: %d files match, want 1" % (label, len(found)))
                continue
            path, data = found[0]
            self.selected[i] = path
            for name, field in sorted(f.get("bind", {}).items()):
                value = data.get(field) if isinstance(data, dict) else None
                if not isinstance(value, str) or not normalise.UUID_RE.fullmatch(value):
                    self.problem("%s: field %s is not a UUID; cannot bind %s" % (label, field, name))
                elif self.bindings.setdefault(name, value) != value:
                    # A name bound by two files must name one UUID: e.g. a shared group_id.
                    self.problem("%s: %s=%s differs from the UUID already bound to %s"
                                 % (label, field, value, name))

    def check_files(self):
        listed = []
        for dirpath, dirnames, filenames in os.walk(self.home):
            dirnames.sort()
            for name in filenames:
                p = os.path.join(dirpath, name)
                listed.append((os.lstat(p).st_mtime_ns, os.path.relpath(p, self.home)))
        actual = sorted(self.norm.text(rel) for _, rel in sorted(listed))
        expect = self.resolve(self.sc["expect"])
        want = sorted(expect["home_files"])
        if actual != want:
            self.problem("home files differ\n  expected: %r\n  actual:   %r" % (want, actual))
        self.check_dirs(expect)
        if not expect.get("files"):
            return
        index = {}
        for dirpath, dirnames, filenames in os.walk(self.root):
            dirnames[:] = sorted(d for d in dirnames if os.path.join(dirpath, d) != self.e2e)
            for name in filenames:
                p = os.path.join(dirpath, name)
                index.setdefault(self.norm.text(p), p)
        expect = self.resolve(self.sc["expect"])
        for i, f in enumerate(expect["files"]):
            if "glob" in f:
                real = self.selected.get(i)
                if real is None:
                    continue  # bind_files reported it
                f = dict(f, path="%s (selected by %s)" % (self.norm.text(real), f["glob"]))
            else:
                real = index.get(f["path"])
            if "absent" in f:
                if real is not None:
                    self.problem("file %s exists, want absent" % f["path"])
                continue
            if real is None:
                self.problem("file %s missing" % f["path"])
                continue
            if "sha256" in f:
                # Raw bytes, never normalised: proves a file was left exactly as it was.
                with open(real, "rb") as fh:
                    got = hashlib.sha256(fh.read()).hexdigest()
                if got != f["sha256"]:
                    self.problem("file %s sha256 %s, want %s" % (f["path"], got, f["sha256"]))
                continue
            content = _read(real)
            if "text" in f:
                got = self.norm.text(content)
                if got != f["text"]:
                    self.problem("file %s differs\n%s" % (f["path"], _diff(f["text"], got)))
                continue
            try:
                got = self.norm.json(json.loads(content))
            except ValueError as err:
                self.problem("file %s is not JSON: %s" % (f["path"], err))
                continue
            if not normalise.matches(f["json"], got):
                self.problem("file %s JSON differs\n  expected subset: %s\n  actual: %s"
                             % (f["path"], json.dumps(f["json"], sort_keys=True), json.dumps(got, sort_keys=True)))

    def check_dirs(self, expect):
        """Exact entry names of a directory: proves it exists and holds nothing else."""
        for d in expect.get("dirs", []):
            real = self.path(d["path"])
            if not os.path.isdir(real) or os.path.islink(real):
                self.problem("dir %s missing" % d["path"])
                continue
            got = sorted(self.norm.text(name) for name in os.listdir(real))
            if got != sorted(d["entries"]):
                self.problem("dir %s entries differ\n  expected: %r\n  actual:   %r"
                             % (d["path"], sorted(d["entries"]), got))

    def check_calls(self):
        want = self.sc["expect"].get("calls", {})
        for name in fakecli.NAMES:
            records = []
            log = fakecli.calls_path(self.root, name)
            if os.path.exists(log):
                with open(log, encoding="utf-8") as f:
                    records = [json.loads(line) for line in f if line.strip()]
            calls = [r for r in records if "event" not in r]
            for r in records:
                if "unsafe" in r:
                    self.result.unsafe.append("fake %s: %s" % (name, r["unsafe"]))
                if "event" in r:
                    self.problem("fake %s: %s on %s" % (name, r["event"], r.get("path")))
                if r.get("error"):
                    self.problem("fake %s: %s" % (name, r["error"]))
                for v in r.get("violations", []):
                    self.problem("fake %s %s: %s" % (name, r["argv"], v))
            if len(calls) != want.get(name, 0):
                self.problem("fake %s called %d times, want %d: %s"
                             % (name, len(calls), want.get(name, 0), [c["argv"] for c in calls]))
            for i, r in enumerate(self.scripts.get(name, {}).get("responses", [])):
                used = sum(1 for c in calls if c.get("response") == i)
                if "times" in r and used != r["times"]:
                    self.problem("fake %s responses[%d] used %d times, want %d" % (name, i, used, r["times"]))

    def check_http(self):
        want = [(r["method"], r["path"]) for r in self.sc["expect"].get("http_requests", [])]
        got = [(r["method"], r["path"]) for r in self.server.requests]
        if sorted(want) != sorted(got):
            self.problem("update server requests differ\n  expected: %r\n  actual:   %r" % (sorted(want), sorted(got)))

    # -- driver

    def cleanup(self):
        """Kills every tracked step still running, then the token sweep. Safe to repeat."""
        for name in sorted(self.handles):
            self.problem("handle %s still running at the end; killed" % name)
        self.handles.clear()
        for handle in self.live:
            if handle.popen.poll() is None:
                handle.kill()
                handle.popen.wait()
        leaked = owned_pids(self.token)
        if leaked:
            self.problem("leaked task processes %s; killed" % leaked)
            kill_owned(self.token)
            if owned_pids(self.token):
                self.problem("task processes survived SIGKILL: %s" % owned_pids(self.token))
        if self.server is not None and not self.server_closed:
            self.server_closed = True
            self.server.close()

    def run(self):
        try:
            try:
                self.setup()
                if self.result.unsafe:
                    return self.result
                self.server = UpdateServer(self.sc.get("update_server", {}).get("routes", {}))
                self.deadline = time.monotonic() + self.sc.get("timeout", DEFAULT_SCENARIO_TIMEOUT)
                self.run_steps()
            except (ScenarioError, OSError) as err:
                self.problem("%s: %s" % (type(err).__name__, err))
            finally:
                with SIGNALS.deferred():
                    self.cleanup()
            # Step output is read only now: no task process can still write to it.
            if self.server is not None:
                self.bind_files()
            checks = [(handle.label, lambda h=handle, e=expect: self.check_output(h, e))
                      for handle, expect in self.outputs]
            if self.server is not None:
                checks += [("calls", self.check_calls), ("files", self.check_files), ("http", self.check_http)]
            for label, check in checks:
                try:
                    check()
                except normalise.NormaliseError as err:
                    self.problem("%s: %s" % (label, err))
        except Interrupted as sig:
            # The signal may have landed before the finally block ran its cleanup.
            with SIGNALS.deferred():
                self.cleanup()
            self.result.interrupted = sig.signum
            self.problem("interrupted by %s; task processes cleaned up" % sig.name)
        return self.result


def run_scenario(sc, bin_path, work_dir, parent_env=None, python=sys.executable):
    root = tempfile.mkdtemp(prefix=re.sub(r"[^A-Za-z0-9_.-]", "_", sc["name"]) + "-", dir=work_dir)
    env = os.environ if parent_env is None else parent_env
    return Task(sc, bin_path, root, env, python).run()


def select(paths, patterns):
    if not patterns:
        return paths
    return [p for p in paths if any(fnmatch.fnmatch(os.path.splitext(os.path.basename(p))[0], g) for g in patterns)]


def main(argv=None):
    ap = argparse.ArgumentParser(description="Run e2e scenarios against a debug rival binary.")
    ap.add_argument("--bin", required=True, help="debug rival binary (honours RIVAL_UPDATE_API)")
    ap.add_argument("--scenario", action="append", default=[], help="glob on scenario file stems; repeatable")
    ap.add_argument("--scenarios-dir", default=SCENARIOS_DIR)
    ap.add_argument("--work-dir", help="parent for per-scenario temp roots (default: a new temp dir)")
    args = ap.parse_args(argv)
    bin_path = os.path.abspath(args.bin)
    if not (os.path.isfile(bin_path) and os.access(bin_path, os.X_OK)):
        ap.error("--bin %s is not an executable file" % args.bin)
    paths = select(sorted(glob.glob(os.path.join(args.scenarios_dir, "*.yaml"))), args.scenario)
    if not paths:
        ap.error("no scenario matches")
    work_dir = args.work_dir or tempfile.mkdtemp(prefix="rival-e2e-")
    os.makedirs(work_dir, exist_ok=True)
    previous = SIGNALS.install()
    # `kill -USR1 <runner>` prints every thread's stack to stderr: evidence for a stalled run.
    faulthandler.register(signal.SIGUSR1, all_threads=True)
    failed = 0
    try:
        for path in paths:
            try:
                sc = load_scenario(path)
            except ScenarioError as err:
                print("FAIL %s\n    %s" % (os.path.basename(path), err), flush=True)
                failed += 1
                continue
            result = run_scenario(sc, bin_path, work_dir)
            if result.ok:
                print("PASS %s" % result.name, flush=True)
                continue
            failed += 1
            print("FAIL %s (root %s)" % (result.name, result.root))
            for msg in result.unsafe:
                print("    UNSAFE " + msg.replace("\n", "\n    "))
            for msg in result.problems:
                print("    " + msg.replace("\n", "\n    "), flush=True)
            if result.interrupted is not None:
                raise Interrupted(result.interrupted)
            if result.unsafe:
                print("unsafe report: run stopped")
                return 3
    except Interrupted as sig:
        print("interrupted by %s: run stopped; roots kept under %s" % (sig.name, work_dir), flush=True)
        return 128 + sig.signum
    finally:
        faulthandler.unregister(signal.SIGUSR1)
        SIGNALS.restore(previous)
    print("%d/%d scenarios passed; roots kept under %s" % (len(paths) - failed, len(paths), work_dir))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
