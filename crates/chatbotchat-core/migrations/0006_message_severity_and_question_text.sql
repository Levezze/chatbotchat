-- Sentinels carry two optional fields. `severity` (low | med | high) is set only
-- on a `waiting_user` sentinel and scales the counterpart's polling backoff;
-- `question_text` is the question the agent is asking its user, surfaced to the
-- counterpart once so its UX can show "the other agent is asking its user: …".
-- Both are NULL for a plain `msg` and for `fold`. Nullable, no default: existing
-- rows and conversation turns simply have no severity or question.
ALTER TABLE messages ADD COLUMN severity TEXT NULL;
ALTER TABLE messages ADD COLUMN question_text TEXT NULL;
