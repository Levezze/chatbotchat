#!/usr/bin/env python3
"""Disposable host-metadata probe, not an authenticated CBC adapter.

No credentials or native IDs are emitted. Identity evidence is hashed and must be
corroborated by the native test chat; synthetic protocol tests do not establish binding.
"""
import argparse
import hashlib
import json
import os
import stat
import sys


def observation(params):
    metadata = params.get("_meta", {})
    if not isinstance(metadata, dict):
        metadata = {}
    claims = {}
    for key in ("threadId", "thread_id"):
        value = metadata.get(key)
        if isinstance(value, str) and 0 < len(value) <= 256:
            claims["meta." + key] = hashlib.sha256(value.encode()).hexdigest()
    for key in ("CODEX_THREAD_ID", "CODEX_SESSION_ID"):
        value = os.environ.get(key)
        if value:
            claims["env." + key] = hashlib.sha256(value.encode()).hexdigest()
    return {"metadata_keys": sorted(str(k)[:128] for k in metadata), "claims_sha256": claims,
            "status": "observed" if claims else "unbound"}


def append_private(path, record):
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600:
            raise ValueError("unsafe probe log")
        os.write(fd, (json.dumps(record, separators=(",", ":")) + "\n").encode())
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", required=True)
    parser.add_argument("--environment-only", action="store_true", help="One-shot native shell evidence; not an MCP result")
    args = parser.parse_args()
    if args.environment_only:
        record = observation({})
        record["transport"] = "native-shell"
        append_private(args.log, record)
        print(json.dumps(record))
        return
    while True:
        line = sys.stdin.buffer.readline(65537)
        if not line:
            return
        if len(line) > 65536:
            raise ValueError("oversized MCP request")
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request.get("method")
        params = request.get("params") or {}
        if method == "initialize":
            result = {"protocolVersion": params.get("protocolVersion", "2024-11-05"),
                      "capabilities": {"tools": {}},
                      "serverInfo": {"name": "cbc-candidate-binding-probe", "version": "0.1.0"}}
        elif method == "tools/list":
            result = {"tools": [{"name": "cbc_candidate_binding_probe",
                      "description": "Disposable host identity evidence probe. Call with no arguments.",
                      "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}}]}
        elif method == "tools/call" and params.get("name") == "cbc_candidate_binding_probe":
            if params.get("arguments") not in ({}, None):
                result = {"isError": True, "content": [{"type": "text", "text": "Probe takes no arguments"}]}
            else:
                record = observation(params)
                append_private(args.log, record)
                result = {"content": [{"type": "text", "text": json.dumps(record)}]}
        elif method == "ping":
            result = {}
        else:
            print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unknown method"}}), flush=True)
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


if __name__ == "__main__":
    main()
