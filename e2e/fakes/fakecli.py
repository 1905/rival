"""Scripted stand-in for the reviewer CLIs, git hosts and Homebrew.

The runner writes one launcher per fake into ``<task root>/bin/<name>``; each
launcher calls ``main(<name>)``. A fake reads ``FAKE_<NAME>_SCRIPT`` (a JSON
file inside ``FAKE_TASK_ROOT``), picks the first response whose argv matcher
fits and whose ``times`` budget is not spent, logs the call to
``<task root>/.e2e/fakes/<name>.calls.jsonl`` and plays the response.

Every failure mode is logged, never guessed around: an unscripted fake, an
argv with no response, an unmet expectation, an unsafe brew prefix and an
expired ``wait_for`` each leave a record the runner turns into a failure.
"""

from __future__ import annotations

import fcntl
import json
import os
import re
import sys
import time

NAMES = ("codex", "claude", "grok", "opencode", "glab", "docker", "brew")

EXIT_WAIT_TIMEOUT = 95
EXIT_UNSCRIPTED = 97
EXIT_UNSAFE = 98

RESPONSE_KEYS = {
    "argv", "argv_prefix", "times", "expect", "delay", "wait_for",
    "wait_timeout", "touch", "stdout", "stderr", "exit",
}
EXPECT_KEYS = {"stdin", "env", "cwd"}


def script_env(name: str) -> str:
    return "FAKE_%s_SCRIPT" % name.upper()


def state_dir(root: str) -> str:
    return os.path.join(root, ".e2e", "fakes")


def calls_path(root: str, name: str) -> str:
    return os.path.join(state_dir(root), name + ".calls.jsonl")


def inside(root: str, path: str) -> bool:
    """Reports whether path resolves strictly below root."""
    if not root or not path or not os.path.isabs(path):
        return False
    real_root = os.path.realpath(root)
    real = os.path.realpath(path)
    return real != real_root and real.startswith(real_root + os.sep)


def value_matches(pattern, value) -> bool:
    """A string matches exactly; {"$regex": r} matches by re.fullmatch."""
    if isinstance(pattern, dict):
        return isinstance(value, str) and re.fullmatch(pattern["$regex"], value) is not None
    return pattern == value


def argv_matches(response: dict, argv: list) -> bool:
    if "argv" in response:
        want = response["argv"]
        return len(want) == len(argv) and all(value_matches(p, a) for p, a in zip(want, argv))
    want = response["argv_prefix"]
    return len(argv) >= len(want) and all(value_matches(p, a) for p, a in zip(want, argv))


def _check_matcher(value, where: str) -> list:
    if isinstance(value, str):
        return []
    if isinstance(value, dict) and set(value) == {"$regex"} and isinstance(value["$regex"], str):
        try:
            re.compile(value["$regex"])
        except re.error as err:
            return ["%s: bad $regex: %s" % (where, err)]
        return []
    return ["%s: want a string or {\"$regex\": ...}" % where]


def validate_script(script) -> list:
    """Returns schema problems; an empty list means the script is usable."""
    if not isinstance(script, dict) or set(script) != {"responses"}:
        return ["script: want exactly {\"responses\": [...]}"]
    responses = script["responses"]
    if not isinstance(responses, list) or not responses:
        return ["script.responses: want a non-empty list"]
    problems = []
    for i, r in enumerate(responses):
        where = "responses[%d]" % i
        if not isinstance(r, dict):
            problems.append(where + ": want an object")
            continue
        unknown = set(r) - RESPONSE_KEYS
        if unknown:
            problems.append("%s: unknown keys %s" % (where, sorted(unknown)))
        if ("argv" in r) == ("argv_prefix" in r):
            problems.append(where + ": set exactly one of argv, argv_prefix")
        for key in ("argv", "argv_prefix"):
            if key in r:
                if not isinstance(r[key], list):
                    problems.append("%s.%s: want a list" % (where, key))
                else:
                    for j, item in enumerate(r[key]):
                        problems += _check_matcher(item, "%s.%s[%d]" % (where, key, j))
        if "exit" not in r or type(r["exit"]) is not int:
            problems.append(where + ".exit: required integer")
        if "times" in r and (type(r["times"]) is not int or r["times"] < 1):
            problems.append(where + ".times: want an integer >= 1")
        for key in ("delay", "wait_timeout"):
            if key in r and (type(r[key]) not in (int, float) or r[key] < 0):
                problems.append("%s.%s: want a number >= 0" % (where, key))
        for key in ("stdout", "stderr", "wait_for", "touch"):
            if key in r and not isinstance(r[key], str):
                problems.append("%s.%s: want a string" % (where, key))
        expect = r.get("expect", {})
        if not isinstance(expect, dict) or set(expect) - EXPECT_KEYS:
            problems.append(where + ".expect: allowed keys %s" % sorted(EXPECT_KEYS))
            continue
        for key in ("stdin", "cwd"):
            if key in expect:
                problems += _check_matcher(expect[key], "%s.expect.%s" % (where, key))
        env = expect.get("env", {})
        if not isinstance(env, dict):
            problems.append(where + ".expect.env: want an object")
        else:
            for k, v in env.items():
                if v is not None:
                    problems += _check_matcher(v, "%s.expect.env.%s" % (where, k))
    return problems


def check_expect(expect: dict, record: dict) -> list:
    violations = []
    if "stdin" in expect and not value_matches(expect["stdin"], record["stdin"]):
        violations.append("stdin %r does not match %r" % (record["stdin"], expect["stdin"]))
    if "cwd" in expect and not value_matches(expect["cwd"], record["cwd"]):
        violations.append("cwd %r does not match %r" % (record["cwd"], expect["cwd"]))
    for key, want in sorted(expect.get("env", {}).items()):
        got = record["env"].get(key)
        if want is None:
            if got is not None:
                violations.append("env %s=%r should be unset" % (key, got))
        elif got is None or not value_matches(want, got):
            violations.append("env %s=%r does not match %r" % (key, got, want))
    return violations


def prefix_problem(root: str, stdout: str):
    """Returns why a `brew --prefix` answer is unsafe, or None when it is safe."""
    prefix = stdout.strip()
    if not inside(root, prefix):
        return "brew prefix %r is outside the task root %r" % (prefix, root)
    return None


class _Locked:
    def __init__(self, path: str):
        self.path = path

    def __enter__(self):
        self.f = open(self.path, "a+")
        fcntl.flock(self.f.fileno(), fcntl.LOCK_EX)
        return self

    def __exit__(self, *exc):
        fcntl.flock(self.f.fileno(), fcntl.LOCK_UN)
        self.f.close()


def _append(path: str, record: dict) -> None:
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(record, sort_keys=True) + "\n")


def _read_stdin() -> str:
    if sys.stdin is None:
        return ""
    try:
        data = sys.stdin.buffer.read()
    except (OSError, ValueError):
        return ""
    return data.decode("utf-8", "surrogateescape")


def _fail(name: str, code: int, message: str) -> int:
    sys.stderr.write("fake %s: %s\n" % (name, message))
    sys.stderr.flush()
    return code


def main(name: str) -> int:
    argv = sys.argv[1:]
    root = os.environ.get("FAKE_TASK_ROOT", "")
    if not root or not os.path.isdir(state_dir(root)):
        return _fail(name, EXIT_UNSCRIPTED, "FAKE_TASK_ROOT is unset or has no .e2e/fakes")
    stdin = _read_stdin()
    record = {
        "pid": os.getpid(),
        "argv": argv,
        "cwd": os.getcwd(),
        "env": dict(os.environ),
        "stdin": stdin,
    }
    log = calls_path(root, name)
    with _Locked(os.path.join(state_dir(root), name + ".lock")):
        script_path = os.environ.get(script_env(name), "")
        if not script_path:
            record["error"] = "unscripted call (%s unset)" % script_env(name)
        elif not inside(root, script_path):
            record["error"] = "script %r is outside the task root" % script_path
        if "error" in record:
            _append(log, record)
            return _fail(name, EXIT_UNSCRIPTED, record["error"])
        with open(script_path, encoding="utf-8") as f:
            script = json.load(f)
        problems = validate_script(script)
        if problems:
            record["error"] = "bad script: " + "; ".join(problems)
            _append(log, record)
            return _fail(name, EXIT_UNSCRIPTED, record["error"])
        state_file = os.path.join(state_dir(root), name + ".state.json")
        used = {}
        if os.path.exists(state_file):
            with open(state_file, encoding="utf-8") as f:
                used = json.load(f)
        chosen = None
        for i, r in enumerate(script["responses"]):
            if argv_matches(r, argv) and used.get(str(i), 0) < r.get("times", float("inf")):
                chosen = i
                break
        if chosen is None:
            record["error"] = "no scripted response for argv %r" % (argv,)
            _append(log, record)
            return _fail(name, EXIT_UNSCRIPTED, record["error"])
        used[str(chosen)] = used.get(str(chosen), 0) + 1
        with open(state_file, "w", encoding="utf-8") as f:
            json.dump(used, f)
        response = script["responses"][chosen]
        record["response"] = chosen
        record["violations"] = check_expect(response.get("expect", {}), record)
        if name == "brew" and argv[:1] == ["--prefix"]:
            unsafe = prefix_problem(root, response.get("stdout", ""))
            if unsafe:
                record["unsafe"] = unsafe
        _append(log, record)
    if "unsafe" in record:
        return _fail(name, EXIT_UNSAFE, record["unsafe"])

    if "touch" in response:
        open(response["touch"], "a").close()
    if "wait_for" in response:
        deadline = time.monotonic() + response.get("wait_timeout", 30)
        while not os.path.exists(response["wait_for"]):
            if time.monotonic() > deadline:
                with _Locked(os.path.join(state_dir(root), name + ".lock")):
                    _append(log, {"event": "wait_timeout", "pid": os.getpid(), "path": response["wait_for"]})
                return _fail(name, EXIT_WAIT_TIMEOUT, "timed out waiting for %s" % response["wait_for"])
            time.sleep(0.02)
    if response.get("delay"):
        time.sleep(response["delay"])
    out = response.get("stdout", "")
    err = response.get("stderr", "")
    if out:
        sys.stdout.buffer.write(out.encode("utf-8", "surrogateescape"))
        sys.stdout.flush()
    if err:
        sys.stderr.buffer.write(err.encode("utf-8", "surrogateescape"))
        sys.stderr.flush()
    return response["exit"]
