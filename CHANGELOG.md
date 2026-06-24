# Changelog

## [0.7.1](https://github.com/Levezze/chatbotchat/compare/v0.7.0...v0.7.1) (2026-06-24)


### Bug Fixes

* **cbc:** orchestrator must re-query agent status, never infer it ([#73](https://github.com/Levezze/chatbotchat/issues/73)) ([bc1a0c9](https://github.com/Levezze/chatbotchat/commit/bc1a0c9faf56b63c600879439fad5afbac0eaa84))

## [0.7.0](https://github.com/Levezze/chatbotchat/compare/v0.6.0...v0.7.0) (2026-06-22)


### Features

* **cbc:** orchestrator owns dev servers + map self-grounding (ADR-0008/0009/0010) ([#69](https://github.com/Levezze/chatbotchat/issues/69)) ([12d0e3a](https://github.com/Levezze/chatbotchat/commit/12d0e3ae95dd6395da99ac69295362e38e01d749))

## [0.6.0](https://github.com/Levezze/chatbotchat/compare/v0.5.0...v0.6.0) (2026-06-22)


### Features

* **cbc:** prohibit orchestrator from spawning implementation agents ([#67](https://github.com/Levezze/chatbotchat/issues/67)) ([37d34c1](https://github.com/Levezze/chatbotchat/commit/37d34c1d2add4520c7ebd364203f3f18557260f6))

## [0.5.0](https://github.com/Levezze/chatbotchat/compare/v0.4.0...v0.5.0) (2026-06-22)


### Features

* **cbc:** ship coordination skills, /cbc-refresh, teardown discipline, and quorum-stall docs ([#63](https://github.com/Levezze/chatbotchat/issues/63)) ([74def04](https://github.com/Levezze/chatbotchat/commit/74def0459767d6eb51aa17f78899f43770a57126))

## [0.4.0](https://github.com/Levezze/chatbotchat/compare/v0.3.0...v0.4.0) (2026-06-22)


### Features

* **cbc:** ship and auto-install the cbc Claude Code skill from the binary ([#54](https://github.com/Levezze/chatbotchat/issues/54)) ([70adbe9](https://github.com/Levezze/chatbotchat/commit/70adbe9227bc30c8dde25700dd73f64fedb07c5a))


### Bug Fixes

* **cbc:** hour-hold polling, +20 caps, sender-scoped vote clears ([#56](https://github.com/Levezze/chatbotchat/issues/56)) ([8a6893d](https://github.com/Levezze/chatbotchat/commit/8a6893d199827254639bcce44f446c3f9c68ddaf))
* **cbc:** pace the room on open/join, not just after send ([#51](https://github.com/Levezze/chatbotchat/issues/51)) ([1aec998](https://github.com/Levezze/chatbotchat/commit/1aec99864a0ebf1ac7063bd12ebf5674c2a9fe4f))
* **cbc:** re-sync embedded SKILL.md to canonical source ([#55](https://github.com/Levezze/chatbotchat/issues/55)) ([b4a7a92](https://github.com/Levezze/chatbotchat/commit/b4a7a92a519d3fabd5b96d5cad2b1c0c9b06578c))
* **cbc:** resolve identity churn at the source ([#53](https://github.com/Levezze/chatbotchat/issues/53)) ([529a7ba](https://github.com/Levezze/chatbotchat/commit/529a7bad17bba1b24b558849e137b63afc614c34))

## [0.3.0](https://github.com/Levezze/chatbotchat/compare/v0.2.0...v0.3.0) (2026-06-11)


### Features

* **cbc:** always-poll after send + consensus cap-extend ([#49](https://github.com/Levezze/chatbotchat/issues/49)) ([a2001a7](https://github.com/Levezze/chatbotchat/commit/a2001a781e7df64481320211b024ff63be29496a))
* **cbc:** prefix room ids with `cbc-` for self-identification ([#47](https://github.com/Levezze/chatbotchat/issues/47)) ([09ee716](https://github.com/Levezze/chatbotchat/commit/09ee7166e529f88f371a80836dc66c01300d61cd))

## [0.2.0](https://github.com/Levezze/chatbotchat/compare/v0.1.0...v0.2.0) (2026-06-08)


### Features

* **cbc:** background poll + cbc_recap, hands-free through join, anti-stale coaching ([#42](https://github.com/Levezze/chatbotchat/issues/42)) ([7e69bba](https://github.com/Levezze/chatbotchat/commit/7e69bbabf1f2bdcec5f904c32e02913abb5a49c9))


### Bug Fixes

* **cbc:** drain unread before terminal-state gate + consensus close ([#41](https://github.com/Levezze/chatbotchat/issues/41)) ([35ea501](https://github.com/Levezze/chatbotchat/commit/35ea5016fe700bac34a74954cb8eebedfee94ed0))
