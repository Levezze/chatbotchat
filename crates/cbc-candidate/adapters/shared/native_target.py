"""Explicit native execution constraints. Placement labels never grant authority."""
import ctypes
import json
import os
import posixpath
import stat
import sys

TARGET_FIELDS = {"v", "profile", "repository", "worktree", "cwd", "saved_project",
                 "saved_environment", "placement"}


def bounded(value, limit=128):
    try:
        return (isinstance(value, str) and bool(value.strip())
                and len(value.encode("utf-8")) <= limit
                and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value))
    except UnicodeEncodeError:
        return False


def normalized_path(value):
    return (bounded(value, 4096) and value.startswith("/")
            and not value.startswith("//") and posixpath.normpath(value) == value)


def within(cwd, worktree):
    return cwd == worktree or cwd.startswith(worktree.rstrip("/") + "/")


def validate_target(target):
    if (not isinstance(target, dict) or set(target) != TARGET_FIELDS
            or type(target["v"]) is not int or target["v"] != 1
            or target["profile"] not in ("desktop", "cli", "unknown")
            or not bounded(target["repository"]) or not normalized_path(target["cwd"])
            or target["worktree"] is not None and (
                not normalized_path(target["worktree"])
                or not within(target["cwd"], target["worktree"]))):
        raise ValueError("invalid native execution target")
    for name in ("saved_project", "saved_environment"):
        selected = target[name]
        if (not isinstance(selected, dict)
                or selected.get("status") not in ("not_exposed", "configured")
                or set(selected) != ({"status"} if selected.get("status") == "not_exposed"
                                     else {"status", "id"})
                or selected.get("status") == "configured" and not bounded(selected.get("id"))):
            raise ValueError("invalid saved native selector")
    placement = target["placement"]
    if (not isinstance(placement, dict) or not {"evidence_ref"} <= set(placement)
            <= {"evidence_ref", "project_label", "environment_label", "project_id", "environment_id"}
            or not all(bounded(value) for value in placement.values())):
        raise ValueError("invalid native placement evidence")
    if len(json.dumps(target, ensure_ascii=False, separators=(",", ":")).encode()) > 16384:
        raise ValueError("native execution target too large")
    return target


def parse_targets(profile, bindings):
    """Parse the same private profile snapshot as its bindings; v1 stays unqualified."""
    version = profile.get("v")
    if type(version) is not int or version not in (1, 2):
        raise ValueError("unsupported profile")
    if version == 2 and set(profile) != {"v", "core_socket", "bindings", "targets"}:
        raise ValueError("invalid native profile fields")
    entries = profile.get("targets")
    if entries is None and version == 1:
        return {session: {"host": "unknown", "target": None} for session in bindings}
    if not isinstance(entries, list):
        raise ValueError("native targets required")
    targets = {}
    for item in entries:
        required = {"session", "host", "target"} if version == 2 else {"session", "host"}
        if (not isinstance(item, dict) or set(item) != required
                or not isinstance(item["session"], str) or item["session"] not in bindings
                or item["session"] in targets or item["host"] not in ("codex", "claude")):
            raise ValueError("invalid native target entry")
        targets[item["session"]] = {"host": item["host"],
            "target": validate_target(item["target"]) if version == 2 else None}
    if set(targets) != set(bindings):
        raise ValueError("every binding requires one native target")
    return targets


def match_target(target, observation):
    if target is None:
        return True  # v1 grants are retained explicitly as weaker legacy bindings.
    try:
        validate_target(target)
        if not isinstance(observation, dict):
            return False
        cwd = observation.get("cwd")
        if (not normalized_path(cwd) or cwd != target["cwd"]
                or target["worktree"] is not None and not within(cwd, target["worktree"])
                or target["profile"] != "unknown" and target["profile"] != observation.get("profile")):
            return False
        for name in ("saved_project", "saved_environment"):
            wanted = target[name]
            if wanted["status"] == "configured" and observation.get(name) != wanted["id"]:
                return False
        return True
    except (ValueError, TypeError, UnicodeError):
        return False


def target_agrees(expected, status):
    """Explicit epoch-bound core authority, including explicit null for legacy grants."""
    if (not isinstance(status, dict) or status.get("type") != "binding_status"
            or "native_target" not in status):
        return False
    actual = status["native_target"]
    if expected is None or actual is None:
        return expected is None and actual is None
    try:
        validate_target(expected)
        validate_target(actual)
        return actual == expected
    except (ValueError, TypeError, UnicodeError):
        return False


if sys.platform == "darwin":
    _libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    _libproc.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                     ctypes.c_void_p, ctypes.c_int]
    _libproc.proc_pidinfo.restype = ctypes.c_int

    # Installed macOS SDK sys/proc_info.h, structs vinfo_stat, vnode_info,
    # vnode_info_path and proc_vnodepathinfo. No subprocess or process environment read.
    class _VinfoStat(ctypes.Structure):
        _fields_ = [("dev", ctypes.c_uint32), ("mode", ctypes.c_uint16),
                    ("nlink", ctypes.c_uint16), ("ino", ctypes.c_uint64),
                    ("uid", ctypes.c_uint32), ("gid", ctypes.c_uint32),
                    ("times_size_blocks", ctypes.c_int64 * 10),
                    ("blksize", ctypes.c_int32), ("flags_gen_rdev", ctypes.c_uint32 * 3),
                    ("qspare", ctypes.c_int64 * 2)]

    class _VnodeInfo(ctypes.Structure):
        _fields_ = [("stat", _VinfoStat), ("type", ctypes.c_int),
                    ("pad", ctypes.c_int), ("fsid", ctypes.c_int32 * 2)]

    class _VnodePath(ctypes.Structure):
        _fields_ = [("info", _VnodeInfo), ("path", ctypes.c_char * 1024)]

    class _VnodePaths(ctypes.Structure):
        _fields_ = [("cwd", _VnodePath), ("root", _VnodePath)]

    def process_cwd(pid):
        if (ctypes.sizeof(_VinfoStat), ctypes.sizeof(_VnodeInfo), ctypes.sizeof(_VnodePath),
                ctypes.sizeof(_VnodePaths), _VnodePath.path.offset) != (136, 152, 1176, 2352, 152):
            return None  # An unsupported ABI cannot qualify a runtime target.
        buf = _VnodePaths()
        size = ctypes.sizeof(buf)
        if _libproc.proc_pidinfo(pid, 9, 0, ctypes.byref(buf), size) != size:
            return None
        raw = bytes(buf.cwd.path)
        try:
            return os.path.realpath(raw.decode("utf-8")) if raw else None
        except UnicodeError:
            return None
else:
    def process_cwd(pid):
        try:
            return os.path.realpath(os.readlink(f"/proc/{pid}/cwd"))
        except OSError:
            return None


def claude_observation(pid, uid, sessions_dir, facts, record_start, adapter_pid=None,
                       cwd_provider=None):
    """Kernel identity plus native host record, never request/model-supplied metadata."""
    if uid != os.getuid() or type(pid) is not int or pid < 1:
        return None
    before = facts(pid)
    if not before or before.get("uid") != uid:
        return None
    child = facts(adapter_pid) if adapter_pid is not None else None
    if adapter_pid is not None and (not child or child.get("uid") != uid or child.get("ppid") != pid):
        return None
    try:
        fd = os.open(os.path.join(sessions_dir, f"{pid}.json"),
                     os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != uid:
                return None
            raw = stream.read(16385)
        record = json.loads(raw) if len(raw) <= 16384 else None
    except (OSError, ValueError):
        return None
    if (not isinstance(record, dict) or type(record.get("pid")) is not int or record["pid"] != pid
            or record_start(record.get("procStart")) != before.get("start")
            or not bounded(record.get("sessionId"), 256)
            or not normalized_path(record.get("cwd"))):
        return None
    cwd_provider = cwd_provider or process_cwd
    cwd = cwd_provider(pid)
    if (cwd != os.path.realpath(record["cwd"]) or not normalized_path(cwd)
            or adapter_pid is not None and cwd_provider(adapter_pid) != cwd
            or facts(pid) != before
            or adapter_pid is not None and facts(adapter_pid) != child):
        return None
    entrypoint = record.get("entrypoint")
    return {"session": record["sessionId"], "cwd": cwd,
            "profile": {"claude-desktop": "desktop", "cli": "cli"}.get(entrypoint, "unknown")
                       if isinstance(entrypoint, str) else "unknown",
            "saved_project": None, "saved_environment": None}
