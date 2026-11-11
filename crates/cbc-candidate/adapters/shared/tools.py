"""The trial's record tools, shared by both host adapters (S1).

Each tool maps to exactly one core operation: closing a room, a work-item operation or a
request operation. The adapter enforces the argument shape it declares, from one table, so
the declared schema and the enforced one cannot drift. The core still decides content,
authority and state; its operations deny unknown fields, and identity never comes from
arguments. The core refuses a member's operation with one generic message whatever the cause,
so the descriptions state each tool's preconditions.

Stdlib only.
"""
import unicodedata

MAX_ID_BYTES = 128
MAX_PAGE = 7  # requests.rs MAX_REQUEST_PAGE
STAGES = ("planning", "implementing", "in_review", "ready_to_merge", "merged", "abandoned")


def _id(description):
    return {"kind": "id", "schema": {"type": "string", "maxLength": MAX_ID_BYTES, "description": description}}


def _text(limit, description, *, multiline=True, blank=False, trimmed=True, nullable=False):
    """Free text as the core checks it: at most limit characters (after trimming, where the core
    trims), never blank unless blank is allowed, and no control characters except a newline in
    multiline text."""
    return {"kind": "text", "limit": limit, "multiline": multiline, "blank": blank, "trimmed": trimmed,
            "nullable": nullable,
            "schema": {"type": ["string", "null"] if nullable else "string", "maxLength": limit,
                       "description": description}}


def _line(limit, description, **options):
    return _text(limit, description, multiline=False, **options)


def _count(description, minimum=0, maximum=None):
    schema = {"type": "integer", "minimum": minimum, "description": description}
    if maximum is not None:
        schema["maximum"] = maximum
    return {"kind": "count", "minimum": minimum, "maximum": maximum, "schema": schema}


def _cursor(description):
    return {"kind": "cursor", "schema": {"type": "string", "maxLength": MAX_ID_BYTES, "description": description}}


def _stage():
    return {"kind": "stage", "schema": {"type": "string", "enum": list(STAGES)}}


KEY = _id("Idempotency key you choose; repeating a call with the same key changes nothing")
ITEM = _id("Work item ID")
REQUEST = _id("Request ID")

PATCH = ("stage", "summary", "next_action", "blocked_operation", "branch")

# name: (description, required fields, optional fields, field specs)
SPECS = {
    "cbc_close": (
        "Close a peer room you belong to, alone, with a one-line reason the other member sees. "
        "The next connect between you opens a replacement room linked to this one.",
        ("room", "reason"), (),
        {"room": _id("Room ID"), "reason": _line(200, "Why the room is closed")}),
    "cbc_work_register": (
        "Register your work item once, at the start of an assignment. Registration is the "
        "start signal; no go-ahead is needed.",
        ("item", "summary", "key"), (),
        {"item": ITEM, "summary": _text(500, "What the item delivers"), "key": KEY}),
    "cbc_work_update": (
        "Revise your work item. Pass the revision you last read; a stale revision is refused "
        "and nothing changes, so re-read with cbc_work_get and retry. An omitted field is left "
        "alone; null clears blocked_operation or branch.",
        ("item", "expected_revision", "key"), PATCH,
        {"item": ITEM, "expected_revision": _count("The item's current revision", 1), "key": KEY,
         "stage": _stage(), "summary": _text(500, "What the item delivers"),
         "next_action": _text(500, "Your next concrete step", blank=True, trimmed=False),
         "blocked_operation": _line(200, "The one operation you are waiting on, or null", trimmed=False,
                                    nullable=True),
         "branch": _line(200, "Your branch, or null", trimmed=False, nullable=True)}),
    "cbc_work_hitl_on": (
        "Primarch only: turn on the human-review flag for a Chapter Master's work item, so its "
        "PR waits for the user's review before merge. Only the user can turn it off.",
        ("item", "key"), (),
        {"item": ITEM, "key": KEY}),
    "cbc_work_get": (
        "Read a work item: your own, or, as a Primarch, any item in your repository.",
        ("item",), (),
        {"item": ITEM}),
    "cbc_request_open": (
        "Ask a question that needs an answer before you can continue. to is \"user\" for the "
        "user, who answers from the dashboard, or a member ID. The answer wakes you; read it "
        "with cbc_request_answers. A question for a member needs an open room with them, so "
        "cbc_connect first. Never route a question for the user through another member.",
        ("to", "question", "key"), ("item",),
        {"to": _id("\"user\" or the member ID who should answer"),
         "question": _text(4000, "The question, self-contained"), "key": KEY,
         "item": _id("The work item the question concerns")}),
    "cbc_request_answer": (
        "Answer a request addressed to you. On a request addressed to the user, this records "
        "that you are passing on what the user told you in this chat: it is kept as relayed, "
        "never as the user's own answer.",
        ("request", "body", "key"), (),
        {"request": REQUEST, "body": _text(4000, "The answer"), "key": KEY}),
    "cbc_request_withdraw": (
        "Withdraw a request you opened that no longer needs an answer.",
        ("request", "reason"), (),
        {"request": REQUEST, "reason": _line(200, "Why it is withdrawn")}),
    "cbc_request_applied": (
        "Record that you have acted on an answer to your request.",
        ("request", "answer"), (),
        {"request": REQUEST, "answer": _count("The answer's id", 1)}),
    "cbc_request_get": (
        "Read one request you opened or that is addressed to you.",
        ("request",), (),
        {"request": REQUEST}),
    "cbc_request_answers": (
        f"Read the answers to a request, oldest first, at most {MAX_PAGE} per page.",
        ("request",), ("after", "limit"),
        {"request": REQUEST, "after": _count("Return answers after this answer id; 0 for the first page"),
         "limit": _count("Page size", 1, MAX_PAGE)}),
    "cbc_request_pending": (
        f"List open and relayed requests you opened or must answer, by ID, at most {MAX_PAGE} "
        "per page.",
        (), ("after", "limit"),
        {"after": _cursor("Return requests after this request ID; empty for the first page"),
         "limit": _count("Page size", 1, MAX_PAGE)}),
}

NAMES = tuple(SPECS)


def definitions():
    """MCP tool definitions, generated from the table the arguments are enforced against."""
    tools = []
    for name, (description, required, optional, fields) in SPECS.items():
        tools.append({"name": name, "description": description,
                      "inputSchema": {"type": "object", "additionalProperties": False,
                                      "required": list(required),
                                      "properties": {f: fields[f]["schema"] for f in (*required, *optional)}}})
    return tools


class ArgumentError(ValueError):
    pass


def _control(c, multiline):
    """Rust's char::is_control: the Unicode Cc category."""
    return unicodedata.category(c) == "Cc" and not (multiline and c == "\n")


def _check(field, spec, value):
    kind = spec["kind"]
    if kind == "count":
        if type(value) is not int or value < spec["minimum"] or (
                spec["maximum"] is not None and value > spec["maximum"]):
            bound = f" and at most {spec['maximum']}" if spec["maximum"] is not None else ""
            raise ArgumentError(f"{field} must be an integer of at least {spec['minimum']}{bound}")
        return
    if kind == "text" and spec["nullable"] and value is None:
        return
    if not isinstance(value, str):
        raise ArgumentError(f"{field} must be a string")
    try:
        size = len(value.encode())
    except UnicodeEncodeError:  # a lone surrogate from a \uD800-style escape
        raise ArgumentError(f"{field} must be valid Unicode")
    if kind == "cursor" and value == "":
        return
    if kind in ("id", "cursor"):
        if not value.strip() or size > MAX_ID_BYTES or any(_control(c, False) for c in value):
            raise ArgumentError(f"{field} must be a nonblank identifier of at most {MAX_ID_BYTES} UTF-8 bytes"
                                " with no control characters" + (", or empty" if kind == "cursor" else ""))
    elif kind == "stage":
        if value not in STAGES:
            raise ArgumentError(f"stage must be one of {', '.join(STAGES)}")
    else:
        text = value.strip() if spec["trimmed"] else value
        if not spec["blank"] and not value.strip():
            raise ArgumentError(f"{field} must not be blank")
        if len(text) > spec["limit"]:
            raise ArgumentError(f"{field} must be at most {spec['limit']} characters")
        if any(_control(c, spec["multiline"]) for c in text):
            raise ArgumentError(f"{field} must be one line with no control characters" if not spec["multiline"]
                                else f"{field} must have no control characters other than newlines")


def core_request(name, args):
    """(op, fields) for the core, or ArgumentError. Never adds identity; never forwards a field
    the table does not name."""
    description, required, optional, fields = SPECS[name]
    if not isinstance(args, dict):
        raise ArgumentError("arguments must be an object")
    allowed = set(required) | set(optional)
    if not set(required) <= set(args) <= allowed:
        listed = ", ".join(required) + (f"; optional {', '.join(optional)}" if optional else "")
        raise ArgumentError(f"{name} takes {listed}" if listed else f"{name} takes no arguments")
    for field, value in args.items():
        _check(field, fields[field], value)
    a = args
    if name == "cbc_close":
        return "close", {"room": a["room"], "reason": a["reason"]}
    if name == "cbc_work_register":
        return "work", {"work": {"op": "register", "item": a["item"], "summary": a["summary"], "key": a["key"]}}
    if name == "cbc_work_update":
        patch = {field: a[field] for field in PATCH if field in a}
        if not patch:
            raise ArgumentError(f"cbc_work_update needs at least one of {', '.join(PATCH)}")
        return "work", {"work": {"op": "update", "item": a["item"], "expected_revision": a["expected_revision"],
                                 "key": a["key"], "patch": patch}}
    if name == "cbc_work_hitl_on":
        return "work", {"work": {"op": "set_hitl", "item": a["item"], "on": True, "key": a["key"]}}
    if name == "cbc_work_get":
        return "work", {"work": {"op": "get", "item": a["item"]}}
    if name == "cbc_request_open":
        owner = "user" if a["to"] == "user" else {"member": a["to"]}
        return "request", {"request": {"op": "open", "owner": owner, "item": a.get("item"),
                                       "question": a["question"], "key": a["key"]}}
    if name == "cbc_request_answer":
        return "request", {"request": {"op": "answer", "request": a["request"], "body": a["body"], "key": a["key"]}}
    if name == "cbc_request_withdraw":
        return "request", {"request": {"op": "withdraw", "request": a["request"], "reason": a["reason"]}}
    if name == "cbc_request_applied":
        return "request", {"request": {"op": "applied", "request": a["request"], "answer": a["answer"]}}
    if name == "cbc_request_get":
        return "request", {"request": {"op": "get", "request": a["request"]}}
    if name == "cbc_request_answers":
        return "request", {"request": {"op": "answers", "request": a["request"], "after": a.get("after", 0),
                                       "limit": a.get("limit", MAX_PAGE)}}
    return "request", {"request": {"op": "pending", "after": a.get("after", ""), "limit": a.get("limit", MAX_PAGE)}}


def reply_payload(reply):
    """The model's view of a core reply to one of these operations, or None if it is not one."""
    kind = reply.get("type") if isinstance(reply, dict) else None
    if kind == "closed" and type(reply.get("sequence")) is int:
        return {"sequence": reply["sequence"]}
    if kind == "work" and isinstance(reply.get("result"), dict):
        return {"work": reply["result"]}
    if kind == "request" and isinstance(reply.get("result"), dict):
        return {"request": reply["result"]}
    return None
