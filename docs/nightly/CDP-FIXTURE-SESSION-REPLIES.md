# Session identity in private CDP fixtures

The retained `65beeff` CI run `42495188883` failed the same 13 private pipe
fixture cases in both modes. The minimal library report had 463 passed, 13
failed and 17 ignored; default had 465 passed, 13 failed and 17 ignored.
Formatting and filesystem policy passed. Later integration and Clippy steps
were not reached. Complete failed logs are retained independently.

Owned birth navigation now awaits the exact original reply, whose session must
match its request. The two legacy fake peers omitted `sessionId` on immediate
session-routed replies; their deferred replies already retained it. The fixtures
now echo the actual request session when present and keep root replies unscoped.
Production matching, deadlines, readiness and cleanup are unchanged. The derived
slow fixture's existing `os.write(4,` insertion anchor is preserved.

A private rootless protocol proof launched the actual two corrected fixture
scripts with synthetic CDP requests, private tmpfs profiles, read-only sources
and no network. Both child processes exited normally. Exactly 12 actual replies
were checked: six retained the exact request session and six root replies had
no session. The result SHA-256 is
`a0c2d4e6f8857ea894239a132b2727b6bd2c7e462e6f5c58b7e5f8731104a036`,
retained at `/var/tmp/o3-server/cdp-session-fixture-proof-bxpsazt_/run1/result.json`.
All 2,793 admitted inputs, including 24 reserved executables, were unchanged;
the same client, container name and retained CID were terminal with no deferred
signals or observation errors. No Cargo, runtime copy, target mount, recursive
cleanup or host test was used.

This is synthetic protocol coverage of the real private fixtures, not Chromium,
the Rust suite, or a complete Status proof. The preceding three real Chromium
birth cases retain their separate receipt. New-head ordinary container CI must
validate the functional matrix; the failed old-head workflow is not rerun.
