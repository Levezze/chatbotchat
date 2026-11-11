#!/usr/bin/env python3
"""Unit tests for the shared record tools (S1): the declared schema is the enforced one, and
the adapter refuses what the core would refuse, so a model gets a specific reason instead of
the core's generic refusal. The core stays the authority; tests/model_tools.rs drives these
tools against a real core.
Run: python3 -B -I test_tools.py -v
"""
import importlib.util, os, sys, unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("tools", os.path.join(HERE, "tools.py"))
tools = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tools)

VALID = {
    "cbc_close": {"room": "r1", "reason": "done"},
    "cbc_work_register": {"item": "w1", "summary": "s", "key": "k"},
    "cbc_work_update": {"item": "w1", "expected_revision": 1, "key": "k", "stage": "planning"},
    "cbc_work_hitl_on": {"item": "w1", "key": "k"},
    "cbc_work_get": {"item": "w1"},
    "cbc_request_open": {"to": "user", "question": "q", "key": "k"},
    "cbc_request_answer": {"request": "r1", "body": "a", "key": "k"},
    "cbc_request_withdraw": {"request": "r1", "reason": "moot"},
    "cbc_request_applied": {"request": "r1", "answer": 1},
    "cbc_request_get": {"request": "r1"},
    "cbc_request_answers": {"request": "r1"},
    "cbc_request_pending": {},
}


def refused(name, args):
    try:
        tools.core_request(name, args)
    except tools.ArgumentError as e:
        return str(e)
    return None


def with_(name, **fields):
    return {**VALID[name], **fields}


class DeclaredIsEnforced(unittest.TestCase):
    def test_every_tool_has_a_valid_example(self):
        self.assertEqual(set(VALID), set(tools.NAMES))
        for name, args in VALID.items():
            with self.subTest(name=name):
                self.assertIsNone(refused(name, args))

    def test_schema_fields_are_exactly_the_accepted_fields(self):
        for definition in tools.definitions():
            name, schema = definition["name"], definition["inputSchema"]
            with self.subTest(name=name):
                self.assertFalse(schema["additionalProperties"])
                for field in schema["required"]:
                    args = dict(VALID[name])
                    args.pop(field)
                    self.assertIsNotNone(refused(name, args), field)
                for field in set(schema["properties"]) - set(schema["required"]):
                    self.assertNotIn(field, VALID[name] if name != "cbc_work_update" else ())
                self.assertIsNotNone(refused(name, {**VALID[name], "member": "someone"}))

    def test_declared_length_limits_are_the_enforced_ones(self):
        for definition in tools.definitions():
            name = definition["name"]
            for field, prop in definition["inputSchema"]["properties"].items():
                limit = prop.get("maxLength")
                if limit is None or field in ("after",):
                    continue
                with self.subTest(name=name, field=field):
                    at, over = "a" * limit, "a" * (limit + 1)
                    self.assertIsNone(refused(name, with_(name, **{field: at})))
                    self.assertIsNotNone(refused(name, with_(name, **{field: over})))

    def test_arguments_must_be_an_object(self):
        self.assertIsNotNone(refused("cbc_work_get", ["w1"]))


class CoreRules(unittest.TestCase):
    """Each case mirrors a check in work.rs, requests.rs or the close path."""

    def test_counts_are_integers_not_booleans_and_stay_in_range(self):
        for value in (True, 0, 1.0, "1"):
            self.assertIsNotNone(refused("cbc_request_applied", with_("cbc_request_applied", answer=value)), value)
        self.assertIsNotNone(refused("cbc_request_answers", with_("cbc_request_answers", limit=tools.MAX_PAGE + 1)))
        self.assertIsNone(refused("cbc_request_answers", with_("cbc_request_answers", limit=tools.MAX_PAGE)))

    def test_stage_is_one_of_the_core_stages(self):
        self.assertIsNotNone(refused("cbc_work_update", with_("cbc_work_update", stage="shipped")))
        for stage in tools.STAGES:
            self.assertIsNone(refused("cbc_work_update", with_("cbc_work_update", stage=stage)))

    def test_identifiers_are_nonblank_at_most_128_bytes_and_free_of_control_characters(self):
        for bad in ("", "   ", "€" * 43, "a\x7fb", "a\x85b", "a\nb", "\ud800"):
            self.assertIsNotNone(refused("cbc_work_get", {"item": bad}), repr(bad))
        self.assertIsNone(refused("cbc_work_get", {"item": "€" * 42 + "ab"}))  # 128 bytes

    def test_multiline_text_allows_newlines_counts_characters_and_must_not_be_blank(self):
        self.assertIsNone(refused("cbc_request_open", with_("cbc_request_open", question="line 1\nline 2")))
        self.assertIsNone(refused("cbc_request_open", with_("cbc_request_open", question="😀" * 4000)))
        for bad in ("😀" * 4001, "  \n ", "tab\there", "bell\x07"):
            self.assertIsNotNone(refused("cbc_request_open", with_("cbc_request_open", question=bad)), repr(bad))
        self.assertIsNotNone(refused("cbc_work_register", with_("cbc_work_register", summary=" ")))

    def test_limits_apply_to_the_trimmed_text_as_the_core_trims_first(self):
        self.assertIsNone(refused("cbc_request_answer", with_("cbc_request_answer", body=" " + "a" * 4000 + "\n")))

    def test_next_action_may_be_empty_but_not_control_text(self):
        self.assertIsNone(refused("cbc_work_update", with_("cbc_work_update", next_action="")))
        self.assertIsNone(refused("cbc_work_update", with_("cbc_work_update", next_action="a\nb")))
        self.assertIsNotNone(refused("cbc_work_update", with_("cbc_work_update", next_action="a\tb")))

    def test_reasons_and_lines_are_one_nonblank_line(self):
        for name in ("cbc_close", "cbc_request_withdraw"):
            for bad in ("", "  ", "two\nlines"):
                self.assertIsNotNone(refused(name, with_(name, reason=bad)), (name, bad))
        for bad in ("", " ", "two\nlines"):
            self.assertIsNotNone(refused("cbc_work_update", with_("cbc_work_update", branch=bad)), repr(bad))

    def test_null_clears_a_line_field(self):
        op, fields = tools.core_request("cbc_work_update",
                                        {"item": "w1", "expected_revision": 3, "key": "k", "blocked_operation": None})
        self.assertEqual((op, fields["work"]["patch"]), ("work", {"blocked_operation": None}))

    def test_an_update_needs_at_least_one_change(self):
        self.assertIsNotNone(refused("cbc_work_update", {"item": "w1", "expected_revision": 1, "key": "k"}))

    def test_hitl_is_on_only(self):
        self.assertIsNotNone(refused("cbc_work_hitl_on", with_("cbc_work_hitl_on", on=False)))
        _, fields = tools.core_request("cbc_work_hitl_on", VALID["cbc_work_hitl_on"])
        self.assertIs(fields["work"]["on"], True)

    def test_the_user_and_a_member_are_distinct_owners(self):
        _, user = tools.core_request("cbc_request_open", VALID["cbc_request_open"])
        _, member = tools.core_request("cbc_request_open", with_("cbc_request_open", to="pri"))
        self.assertEqual(user["request"]["owner"], "user")
        self.assertEqual(member["request"]["owner"], {"member": "pri"})


class Replies(unittest.TestCase):
    def test_record_replies_map_to_payloads_and_others_do_not(self):
        self.assertEqual(tools.reply_payload({"type": "closed", "sequence": 9}), {"sequence": 9})
        self.assertEqual(tools.reply_payload({"type": "work", "result": {"type": "item"}}), {"work": {"type": "item"}})
        self.assertEqual(tools.reply_payload({"type": "request", "result": {"type": "opened"}}),
                         {"request": {"type": "opened"}})
        for other in ({"type": "error", "message": "x"}, {"type": "closed", "sequence": True},
                      {"type": "work", "result": "x"}, {"type": "sent", "sequence": 1}, None, []):
            self.assertIsNone(tools.reply_payload(other), other)


if __name__ == "__main__":
    unittest.main()
