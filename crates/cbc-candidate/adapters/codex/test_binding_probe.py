import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ProbeTest(unittest.TestCase):
    def test_host_claim_is_hashed_and_model_identity_arguments_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            log = Path(root) / "observed.jsonl"
            requests = [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "cbc_candidate_binding_probe", "arguments": {}, "_meta": {"threadId": "synthetic-native-id"}}},
                {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "cbc_candidate_binding_probe", "arguments": {"threadId": "model-supplied"}}},
            ]
            result = subprocess.run([sys.executable, "-I", str(Path(__file__).with_name("binding_probe.py")), "--log", str(log)],
                                    input="".join(json.dumps(r) + "\n" for r in requests), text=True, capture_output=True, check=True,
                                    env={k: v for k, v in os.environ.items() if k not in ("CODEX_THREAD_ID", "CODEX_SESSION_ID")})
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertTrue(rows[2]["result"]["isError"])
            observed = json.loads(log.read_text())
            self.assertEqual(observed["claims_sha256"]["meta.threadId"], hashlib.sha256(b"synthetic-native-id").hexdigest())
            self.assertNotIn("synthetic-native-id", result.stdout + log.read_text())
            self.assertEqual(log.stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
