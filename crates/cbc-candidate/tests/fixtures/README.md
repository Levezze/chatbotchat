# Synthetic v1 upgrade fixture

`store-v1.sqlite3` was created on 2026-10-23 by running `capture_v1_fixture` against the actual
candidate v1 library before adding migration 0002 or changing SCHEMA_VERSION. It contains one
synthetic context, `v1-upgrade-kept`, and no user, native-session or daily-CBC data.

Fixture SHA-256: `12e3812c39547ba751de5590647bd829502a7e1a6314f2d32d96374fe1cd1be6`.
Original 0001 SQL SHA-256: `0993e6d4d30e3cb41c9372c9055b6569bfcc8d06c9daede7a289a2c010c325cb`.

The capture tool refuses to run with a newer schema. Do not regenerate this fixture using
the current migrator and call it a v1 database. Upgrade tests copy it to a temporary private
candidate root and set the copy to mode 0600 before opening through the public Store API.

## Synthetic v2 upgrade fixture

`store-v2.sqlite3` was captured on 2026-10-26 with the actual v2 migrator before
adding migration 0003. It contains context `v2-upgrade-kept`, Repo Lead `lead`, and
one synthetic binding at epoch 1. Its random credential was never exported.
SHA-256: `ccedb20881da6e1fcc4b0436a0aceca8292b3edaf089b2ae7d45c1749a9e6199`.
The capture example refuses any newer schema or existing destination.

## Synthetic v3 upgrade fixture

`store-v3.sqlite3` was captured on 2026-11-04 with the actual v3 build before
adding migration 0004. It contains context `v3-upgrade-kept`, participants `one`
and `two` bound at epoch 1, and room `original-room`. No native or daily data
was used, and its random credentials were not retained.
SHA-256: `88dea257a15ca375e3f77e249cd51971b1bae856810b0c370f9bae8fe3c6a859`.
The capture example refuses any newer schema or existing destination.
