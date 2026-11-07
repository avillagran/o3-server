# Test filesystem safety audit

Runtime tests execute only in a private rootless container with read-only source
and binaries, private temporary storage and no network or host credentials.
Android compilation and execution use `android-sandbox` exclusively.

Fixture workspaces remain until container teardown. Tests must not recursively
delete trees. Filesystem link fixtures target only their own private workspace;
executable fixture setup copies dependencies rather than linking to host paths.
Production deletion behavior needs an explicit planning/recording boundary so
tests can verify ownership, refusal and operation ordering without deleting trees.
The production executor remains real and separately reviewed.

`tools/check-test-safety` detects explicit recursive cleanup identifiers in Rust
integration tests and test scopes. It ignores inert comments and strings. This
lexical check does not establish transitive filesystem safety or inspect every
external script; those paths require review. Six pure scanner regressions run
without creating or removing fixture files.

When present, `tests/SAFETY-AUDIT-PENDING` prevents complete runtime matrices through
the local runner and CI. On 2026-11-06, the marker was removed after independent
and parent review of the default server Cargo matrix and CI Python tools. The
four destructive Git consumers have explicit retaining test executors; browser
clear uses a recording plan, and auto-cleaning tool directories retain fixtures.
Owned link targets and indirect cleanup paths were reviewed. This admits a new
private complete-matrix verification; it does not turn earlier runs into policy
acceptance. Desktop/QML, ignored live providers, real SDKs and hardware remain
separate acceptance scopes. Reintroduce the marker if a new audit blocker appears.
Use `--integration TARGET --case-filter EXACT_TEST_NAME` to run a single reviewed
integration case without copying or bypassing the guarded container runner.

Earlier full-suite counts are historical functional evidence. Legacy recursive
fixture cleanup was discovered after those runs. Container isolation does not
make those runs acceptance evidence for this policy. No unexpected deletion
outside the disposable test environment was observed; broad execution was stopped
when the audit identified the violation.
