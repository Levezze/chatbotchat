"""The driver's structured log (B2): one JSON object per line, schema v1, the core's schema.

Every line starts with v, t_ms, level, component, run, instance and event, then the event's own
fields. Only the names in FIELDS (the same list as the core's src/log.rs) are written, so a
message body, a token or a path cannot reach the log by accident.

The level is live: the core puts it on every binding_status reply, and the router passes it to
set_level. "normal" writes info and above, "debug" adds debug lines.

The file is private (0600, never through a link, in a 0700 directory), bounded in size and
rotated, keeping `keep` older files. A line that cannot be written is counted, and the count is
written as a log_dropped line once writing works again. Writing never raises.
"""
import hashlib
import json
import os
import re
import secrets
import stat
import threading
import time

SCHEMA = 1
MAX_BYTES = 8 * 1024 * 1024
KEEP = 3
LEVELS = ("normal", "debug")
FIELDS = frozenset([
    "member", "chat", "context", "room", "batch", "reference", "delivery", "kind", "reason",
    "outcome", "count", "seq", "attempt", "targets", "pending", "session", "retry_ms",
])
_RUN = re.compile(r"[A-Za-z0-9._-]{1,64}")


def batch_tag(reference):
    """A receipt reference is a capability, so a log carries this tag instead: the core's
    facts::batch_tag, so driver lines join the core's lines and facts on one batch."""
    return hashlib.sha256(("cbc-batch-tag-v1:" + reference).encode()).hexdigest()[:16]


def run_id(text):
    """A run id is short and plain, so it can sit on every line as is."""
    if not isinstance(text, str) or not _RUN.fullmatch(text):
        raise ValueError("a run id is 1 to 64 letters, digits, '-', '_' or '.'")
    return text


class EventLog:
    def __init__(self, directory, name, component, run=None, max_bytes=MAX_BYTES, keep=KEEP):
        if keep < 1 or max_bytes < 1:
            raise ValueError("a log keeps at least one older file")
        try:
            os.mkdir(directory, 0o700)
        except FileExistsError:
            pass
        st = os.lstat(directory)
        if not stat.S_ISDIR(st.st_mode) or st.st_mode & 0o077:
            raise ValueError("the log directory must be a private directory")
        self.path = os.path.join(directory, name)
        self.component = component
        self.run = run
        self.instance = secrets.token_hex(8)
        self.max_bytes = max_bytes
        self.keep = keep
        self.level = "normal"
        self.dropped = 0
        self.fd = None
        self.size = 0
        self.lock = threading.Lock()
        try:  # open now, so descriptor pressure later cannot cost the log; else retry per write
            self._reopen()
        except OSError:
            pass

    def set_level(self, level):
        if level in LEVELS:
            self.level = level

    def write(self, level, event, fields):
        try:
            if level == "debug" and self.level != "debug":
                return
            line = self._line(level, event,
                              {k: v for k, v in fields.items() if k in FIELDS and v is not None})
            with self.lock:
                if self.dropped:
                    if not self._append(self._line("warn", "log_dropped", {"count": self.dropped})):
                        self.dropped += 1
                        return
                    self.dropped = 0
                if not self._append(line):
                    self.dropped += 1
        except Exception:  # the delivery path must survive anything the log does
            pass

    def close(self):
        with self.lock:
            self._close()

    def _line(self, level, event, fields):
        record = {"v": SCHEMA, "t_ms": int(time.time() * 1000), "level": level,
                  "component": self.component, "run": self.run, "instance": self.instance,
                  "event": event}
        record.update(fields)
        return (json.dumps(record, ensure_ascii=False, separators=(",", ":"), default=str)
                + "\n").encode()

    def _append(self, data):
        try:
            if self.fd is None:
                self._reopen()
            if self.size > 0 and self.size + len(data) > self.max_bytes:
                self._rotate()
            view = memoryview(data)
            while view:
                view = view[os.write(self.fd, view):]
            self.size += len(data)
            return True
        except OSError:
            # A write can fail partway, on a full disk say. Cut the file back to the last whole
            # line, so the next line does not join a broken one; then reopen at the next line,
            # since the file may also have been moved or replaced.
            if self.fd is not None:
                try:
                    os.ftruncate(self.fd, self.size)
                except OSError:
                    pass
            self._close()
            return False

    def _reopen(self):
        """Never through a link and never blocking (a FIFO with no reader would hang the open);
        only a regular file of ours with one name is kept, so a hardlink to a file elsewhere
        is refused and the log never writes outside its directory. A file an earlier writer
        left readable (the router's old writer made 0644 files) is made private through this
        descriptor, not refused, so an upgraded router keeps logging."""
        fd = os.open(self.path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW
                     | os.O_NONBLOCK | os.O_CLOEXEC, 0o600)
        try:
            st = os.fstat(fd)
            if (not stat.S_ISREG(st.st_mode) or st.st_nlink != 1
                    or st.st_uid != os.geteuid()):
                raise OSError("the log file must be a regular file of ours with one name")
            if st.st_mode & 0o077:
                os.fchmod(fd, 0o600)
                st = os.fstat(fd)
        except OSError:
            os.close(fd)
            raise
        self.fd, self.size = fd, st.st_size

    def _close(self):
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None

    def _rotate(self):
        """name.(keep-1) -> name.keep, ..., name -> name.1; the oldest falls off."""
        self._close()
        for n in range(self.keep - 1, 0, -1):
            try:
                os.rename(f"{self.path}.{n}", f"{self.path}.{n + 1}")
            except FileNotFoundError:
                pass
        os.rename(self.path, f"{self.path}.1")
        self._reopen()
