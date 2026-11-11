"""The driver's structured log (B2): schema v1, shared with the core's log."""
import importlib.util
import json
import os
import pathlib
import shutil
import stat
import tempfile
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

    def test_a_run_id_is_short_and_plain(self):
        self.assertEqual(log.run_id("trial-1.a_b"), "trial-1.a_b")
        for bad in ("", "x" * 65, "a b", "a/b", "é"):
            with self.assertRaises(ValueError):
                log.run_id(bad)


if __name__ == "__main__":
    unittest.main()
