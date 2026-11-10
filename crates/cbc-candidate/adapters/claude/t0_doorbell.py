#!/usr/bin/env python3
"""T0: post one plain-text doorbell line to a Claude Code session inbox socket.

Disposable transport probe. Sends no auth line (this process is not the target's
child and must not claim to be), reads any reply briefly, prints timing as JSON.
Usage: python3 -I t0_doorbell.py --session-id <local_...> --text "CBC_T0 n1"
The socket path is resolved from ~/.claude/sessions/<pid>.json by hostSessionId.
"""
import argparse, glob, json, os, socket, time


def resolve(host_session_id):
    for f in glob.glob(os.path.expanduser("~/.claude/sessions/*.json")):
        try:
            d = json.load(open(f))
        except (OSError, ValueError):
            continue
        if d.get("hostSessionId") == host_session_id:
            return d.get("messagingSocketPath"), d.get("pid"), d.get("status")
    return None, None, None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--session-id", required=True)
    ap.add_argument("--text", required=True)
    a = ap.parse_args()
    if not a.text.startswith("CBC_T0 ") or len(a.text) > 200:
        raise SystemExit("refusing: test text must start with 'CBC_T0 ' and be short")
    path, pid, status = resolve(a.session_id)
    out = {"session": a.session_id, "pid": pid, "status_before": status, "socket_found": bool(path)}
    if not path:
        print(json.dumps(out)); return
    frame = json.dumps({"type": "user", "message": {"role": "user", "content": a.text}}) + "\n"
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(5.0)
    t0 = time.time()
    try:
        s.connect(path)
        s.sendall(frame.encode())
        out["written_at"] = t0
        out["bytes"] = len(frame)
        s.settimeout(1.5)
        try:
            out["reply"] = s.recv(4096).decode(errors="replace")[:300]
        except socket.timeout:
            out["reply"] = None
    except OSError as e:
        out["error"] = f"{type(e).__name__}: {e}"
    finally:
        s.close()
    print(json.dumps(out))


if __name__ == "__main__":
    main()
