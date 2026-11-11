"""The driver's structured log (B2): schema v1, shared with the core's log."""
import ast
import importlib.util
import json
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest

spec = importlib.util.spec_from_file_location("log", pathlib.Path(__file__).with_name("log.py"))
log = importlib.util.module_from_spec(spec)
spec.loader.exec_module(log)


def lines(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f.read().splitlines()]


class EventLogTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cbcl-", dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.dir = os.path.join(self.tmp, "logs")
        self.path = os.path.join(self.dir, "router-events.jsonl")

    def open(self, **kw):
        return log.EventLog(self.dir, "router-events.jsonl", "driver", run=kw.pop("run", "run-7"), **kw)

    def test_a_line_carries_the_v1_header_and_only_allowlisted_fields(self):
        events = self.open()
        events.write("info", "hello", {"member": "lead", "count": 2, "pending": True,
                                       "body": "private words", "token": "cbcc1_secret"})
        [line] = lines(self.path)
        self.assertEqual(line["v"], 1)
        self.assertIsInstance(line["t_ms"], int)
        self.assertEqual(line["level"], "info")
        self.assertEqual(line["component"], "driver")
        self.assertEqual(line["run"], "run-7")
        self.assertEqual(len(line["instance"]), 16)
        self.assertEqual(line["event"], "hello")
        self.assertEqual((line["member"], line["count"], line["pending"]), ("lead", 2, True))
        self.assertNotIn("body", line)
        self.assertNotIn("token", line)
        with open(self.path, encoding="utf-8") as f:
            text = f.read()
        self.assertNotIn("private words", text)
        self.assertNotIn("cbcc1_secret", text)

    def test_the_header_names_match_the_core_log(self):
        self.assertEqual(log.SCHEMA, 1)
        core = pathlib.Path(__file__).parents[2] / "src" / "log.rs"
        source = core.read_text(encoding="utf-8")
        listed = source.split("pub const FIELDS: &[&str] = &[", 1)[1].split("];", 1)[0]
        rust = {name.strip().strip('"') for name in listed.split(",") if name.strip()}
        self.assertEqual(rust, set(log.FIELDS))

    def test_every_field_an_adapter_logs_is_on_the_allowlist(self):
        # The log drops a field it does not know, so a misnamed one vanishes without an error.
        adapters = pathlib.Path(__file__).parents[1]
        unknown = []
        for source in sorted(adapters.rglob("*.py")):
            if source.name.startswith("test_"):
                continue
            for node in ast.walk(ast.parse(source.read_text(encoding="utf-8"))):
                if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                        and node.func.attr == "event") or (
                        isinstance(node, ast.Call) and getattr(node.func, "id", None) == "event"):
                    unknown += [f"{source.relative_to(adapters)}:{node.lineno} {k.arg}"
                                for k in node.keywords
                                if k.arg and k.arg != "level" and k.arg not in log.FIELDS]
        self.assertEqual(unknown, [])

    def test_debug_lines_are_written_only_while_the_level_is_debug(self):
        events = self.open()
        events.write("debug", "core_call", {"count": 1})
        events.set_level("debug")
        events.write("debug", "core_call", {"count": 2})
        events.set_level("verbose")  # not a level: ignored, debug stays
        events.write("debug", "core_call", {"count": 3})
        events.set_level("normal")
        events.write("debug", "core_call", {"count": 4})
        events.write("warn", "kept", {"count": 5})
        self.assertEqual([l["count"] for l in lines(self.path)], [2, 3, 5])

    def test_the_file_rotates_at_its_bound_and_keeps_two_older_files(self):
        events = self.open(max_bytes=600, keep=2)
        for seq in range(40):
            events.write("info", "delivered", {"seq": seq})
        names = sorted(os.listdir(self.dir))
        self.assertEqual(names, ["router-events.jsonl", "router-events.jsonl.1", "router-events.jsonl.2"])
        for name in names:
            size = os.path.getsize(os.path.join(self.dir, name))
            self.assertTrue(0 < size <= 600, (name, size))
        live, older = lines(self.path), lines(self.path + ".1")
        self.assertEqual(live[-1]["seq"], 39)
        self.assertEqual(older[-1]["seq"] + 1, live[0]["seq"])

    def test_a_line_that_cannot_be_written_is_counted_and_reported_on_the_next_good_line(self):
        os.mkdir(self.dir, 0o700)
        elsewhere = os.path.join(self.tmp, "elsewhere")
        open(elsewhere, "w").close()
        os.symlink(elsewhere, self.path)  # a link at the log's path is refused, never followed
        events = self.open(run=None)
        events.write("info", "lost", {"seq": 1})
        events.write("info", "lost", {"seq": 2})
        os.unlink(self.path)
        events.write("info", "kept", {"seq": 3})
        self.assertEqual(os.path.getsize(elsewhere), 0)
        got = lines(self.path)
        self.assertEqual([l["event"] for l in got], ["log_dropped", "kept"])
        self.assertEqual((got[0]["level"], got[0]["count"]), ("warn", 2))
        self.assertIsNone(got[0]["run"])

    def test_a_hardlink_at_the_log_path_is_refused_and_its_target_never_written(self):
        os.mkdir(self.dir, 0o700)
        elsewhere = os.path.join(self.tmp, "elsewhere")
        fd = os.open(elsewhere, os.O_WRONLY | os.O_CREAT, 0o600)
        os.close(fd)
        os.link(elsewhere, self.path)
        events = self.open()
        events.write("info", "lost", {"seq": 1})
        self.assertEqual(os.path.getsize(elsewhere), 0)
        os.unlink(self.path)
        events.write("info", "kept", {"seq": 2})
        self.assertEqual(os.path.getsize(elsewhere), 0)
        self.assertEqual([l["event"] for l in lines(self.path)], ["log_dropped", "kept"])

    def test_a_fifo_at_the_log_path_is_refused_without_blocking(self):
        os.mkdir(self.dir, 0o700)
        os.mkfifo(self.path, 0o600)
        done = threading.Event()

        def run():
            self.open().write("info", "lost", {"seq": 1})
            done.set()
        threading.Thread(target=run, daemon=True).start()
        self.assertTrue(done.wait(5), "opening a FIFO with no reader must not block the log")

    def test_a_partly_written_line_is_cut_back_before_the_next_line(self):
        # A child process lowers its own file-size limit, so a long line is written in part and
        # then fails, as on a full disk; the limit is lifted before the next line.
        child = textwrap.dedent("""
            import importlib.util, resource, signal, sys
            spec = importlib.util.spec_from_file_location("log", sys.argv[1])
            log = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(log)
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            soft, hard = resource.getrlimit(resource.RLIMIT_FSIZE)
            events = log.EventLog(sys.argv[2], "router-events.jsonl", "driver")
            events.write("info", "first", {})
            resource.setrlimit(resource.RLIMIT_FSIZE, (events.size + 40, hard))
            events.write("info", "lost", {"reason": "x" * 400})
            resource.setrlimit(resource.RLIMIT_FSIZE, (soft, hard))
            events.write("info", "kept", {})
        """)
        subprocess.run([sys.executable, "-I", "-c", child,
                        str(pathlib.Path(__file__).with_name("log.py")), self.dir], check=True)
        self.assertEqual([l["event"] for l in lines(self.path)], ["first", "log_dropped", "kept"])

    def test_a_write_never_raises(self):
        events = self.open()
        shutil.rmtree(self.dir)
        events.close()
        events.write("info", "lost", {"seq": 1})  # the delivery path must survive a lost log

    def test_the_log_is_private_and_a_shared_directory_is_refused(self):
        self.open().write("info", "hello", {})
        self.assertEqual(stat.S_IMODE(os.stat(self.dir).st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(os.stat(self.path).st_mode), 0o600)
        shared = os.path.join(self.tmp, "shared")
        os.mkdir(shared, 0o755)
        os.chmod(shared, 0o755)
        with self.assertRaises(ValueError):
            log.EventLog(shared, "router-events.jsonl", "driver")

    def test_a_reference_is_logged_as_the_cores_batch_tag(self):
        # src/log.rs pins the same value; a receipt reference is a capability, never logged raw.
        self.assertEqual(log.batch_tag("ref-secret-1"), "6292735d22f1737a")

    def test_a_run_id_is_short_and_plain(self):
        self.assertEqual(log.run_id("trial-1.a_b"), "trial-1.a_b")
        for bad in ("", "x" * 65, "a b", "a/b", "é"):
            with self.assertRaises(ValueError):
                log.run_id(bad)


if __name__ == "__main__":
    unittest.main()
