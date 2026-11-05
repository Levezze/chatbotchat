"""Content-free native Mod wake HTTP on the shared device Unix socket."""
import hashlib
import json
import os
import re
import stat
import time

MAX_HEADERS = 8192
HEADER_SECONDS = 2.0
WAIT_SECONDS = 25.0
MAX_WAITERS = 16
REQUEST = re.compile(rb"GET /cbc/v1/wake\?after=([0-9a-f]{64}|) HTTP/1\.[01]")
HEADER_NAME = re.compile(rb"[!#$%&'*+.^_`|~0-9A-Za-z-]+")


def read_request(conn, initial=b""):
    """One bounded, body-free HTTP request. No keep-alive or pipelining."""
    deadline = time.monotonic() + HEADER_SECONDS
    data = initial
    while b"\r\n\r\n" not in data:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or len(data) >= MAX_HEADERS:
            raise ValueError("invalid headers")
        conn.settimeout(remaining)
        part = conn.recv(min(1024, MAX_HEADERS + 1 - len(data)))
        if not part:
            raise ValueError("incomplete headers")
        data += part
    if len(data) > MAX_HEADERS or not data.endswith(b"\r\n\r\n"):
        raise ValueError("body or oversized headers")
    lines = data[:-4].split(b"\r\n")
    match = REQUEST.fullmatch(lines[0])
    if not match:
        raise ValueError("invalid wake request")
    headers = {}
    for line in lines[1:]:
        name, separator, value = line.partition(b":")
        if (not separator or not HEADER_NAME.fullmatch(name) or name.lower() in headers
                or any(c < 32 and c != 9 or c > 126 for c in value)):
            raise ValueError("invalid header")
        headers[name.lower()] = value.strip()
    if (b"transfer-encoding" in headers or headers.get(b"content-length", b"0") != b"0"
            or headers.get(b"expect") is not None):
        raise ValueError("request body unsupported")
    return match[1].decode("ascii")


def response(status, body=None):
    names = {200: "OK", 204: "No Content", 400: "Bad Request", 401: "Unauthorized",
             429: "Too Many Requests", 503: "Service Unavailable"}
    payload = b"" if body is None else json.dumps(body, separators=(",", ":")).encode()
    return (f"HTTP/1.1 {status} {names[status]}\r\nContent-Length: {len(payload)}\r\n"
            "Content-Type: application/json\r\nConnection: close\r\n\r\n").encode() + payload


def corroborate_native(pid, uid, sessions_dir, facts, record_start):
    """The Mod is in the native host. Never use the shim-parent identity path."""
    if uid != os.getuid() or type(pid) is not int or pid < 1:
        return None
    before = facts(pid)
    if not before or before.get("uid") != uid:
        return None
    try:
        fd = os.open(os.path.join(sessions_dir, f"{pid}.json"),
                     os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != uid:
                return None
            raw = stream.read(16385)
        if len(raw) > 16384:
            return None
        record = json.loads(raw)
    except (OSError, ValueError):
        return None
    if not isinstance(record, dict):
        return None
    session = record.get("sessionId")
    if (type(record.get("pid")) is not int or record["pid"] != pid
            or record_start(record.get("procStart")) != before.get("start")
            or not isinstance(session, str) or not session or facts(pid) != before):
        return None
    return session


def cursor(binding, first):
    # Domain-separated opaque equality token. First unread may move DOWN when an
    # older room joins a member; the global watch cursor cannot represent this.
    value = json.dumps(["cbc-native-wake-v1", *[binding[key] for key in
        ("context", "member", "device", "session", "epoch")], first], separators=(",", ":"))
    return hashlib.sha256(value.encode()).hexdigest()
