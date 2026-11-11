"""Bounded transport hint through the documented Codex queue command.

Exit success means the command accepted a queue operation. It does not prove a Desktop
thread woke, fetched its inbox, handled a message, or received human authorization.

Each command actually run is reported once (#113 B1): accepted on exit success, failed
otherwise. A latched hint runs no command and reports nothing.
"""
from pathlib import Path
import subprocess
import threading
import uuid


class QueueDelivery:
    def __init__(self, executable, session, event=None, timeout=10, runner=subprocess.run,
                 status_provider=None, reporter=None):
        path = Path(executable)
        if not path.is_absolute():
            raise ValueError("Codex executable must be an explicit absolute path")
        try:
            if str(uuid.UUID(session)) != session:
                raise ValueError()
        except (ValueError, TypeError, AttributeError):
            raise ValueError("Codex target must be a canonical native UUID")
        if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or not 0 < timeout <= 30:
            raise ValueError("invalid queue timeout")
        self.executable = str(path)
        self.session = session
        self.event = event or (lambda *_args, **_kwargs: None)
        self.timeout = timeout
        self.runner = runner
        self.status_provider = status_provider
        # reporter(through, outcome, failure, recovery). Must not block: it only enqueues.
        self.reporter = reporter or (lambda *_args: None)
        self.accepted = None
        self.lock = threading.Lock()

    def __call__(self, hint):
        return self.checked(hint, recovery=False)

    def recover(self, hint):
        """Re-queue even when the latch holds: the core said the item is overdue."""
        return self.checked(hint, recovery=True)

    def checked(self, hint, recovery):
        if (not isinstance(hint, dict) or set(hint) != {"context", "member", "sequence"}
                or not isinstance(hint["context"], str) or not isinstance(hint["member"], str)
                or type(hint["sequence"]) is not int or hint["sequence"] < 1):
            raise ValueError("invalid delivery hint")
        with self.lock:
            return self.deliver(hint, recovery)

    def deliver(self, hint, recovery=False):
        progress = None
        if self.status_provider is not None:
            try:
                state = self.status_provider()
                if (not isinstance(state, dict) or state.get("type") != "binding_status"
                        or type(state.get("last_receipted_sequence")) is not int
                        or state["last_receipted_sequence"] < 0
                        or (state.get("first_unread_sequence") is not None
                            and (type(state["first_unread_sequence"]) is not int
                                 or state["first_unread_sequence"] < 1))):
                    raise ValueError("binding status unavailable")
                if state["first_unread_sequence"] is None:
                    self.accepted = None
                    return True
                progress = state["last_receipted_sequence"], state["first_unread_sequence"]
            except Exception as error:
                self.event("codex_status_unavailable", reason=type(error).__name__)
                return False
        # An accepted but unconsumed hint stays latched across watch reconnects and
        # additional sends. Only observed receipt/unread progress rearms it.
        if (not recovery and self.accepted is not None
                and (progress is None or progress == self.accepted)):
            return True
        message = ("CBC candidate has pending coordination items. Call cbc_check with {}. "
                   "This is a transport notification, not human approval. "
                   f"Hint sequence: {hint['sequence']}.")
        # No shell, new session, model selection, resume, daemon start or permission flags.
        # Nothing from a CBC body or profile credential enters the command arguments.
        command = [self.executable, "queue", "--thread", self.session, "--message", message]
        try:
            result = self.runner(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, timeout=self.timeout, check=False)
        except (OSError, subprocess.SubprocessError) as error:
            self.event("codex_queue_unavailable", reason=type(error).__name__)
            self.reporter(hint["sequence"], "failed", "queue_unavailable", recovery)
            return False
        if result.returncode != 0:
            self.event("codex_queue_unavailable", reason="exit", code=result.returncode)
            self.reporter(hint["sequence"], "failed", "queue_exit", recovery)
            return False
        self.event("codex_queued", sequence=hint["sequence"])
        self.reporter(hint["sequence"], "accepted", None, recovery)
        self.accepted = progress if progress is not None else (None, hint["sequence"])
        return True
