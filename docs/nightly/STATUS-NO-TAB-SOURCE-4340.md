# Private original no-tab Status continuation

The bounded private no-tab continuation has passed the checks below. This is
not acceptance of complete Status or full browser parity.
The public selector and browser bootstrap remain closed. Existing tabs still
refuse Status until all actual producers are observed.

The actual `OwnedPageFactory` admits the one-use source-issued Status callback,
proves the actual explicit selection or every possible implicit latest-thread
selection absent using the raw registry. Catalog capture occurs while that
registry lock is held, giving one absence/catalog linearization point. Closing or unready tabs cannot turn
into a fabricated no-tab response. No Page worker is started. There is no
Control, RunContext, generation grant, model-supplied catalog or model-selected
reply variant. Caller validity is checked again after catalog capture through `callback_current(Status)`,
which checks the actual issued capability, current caller scope and available
browser authority. This preserves original read-only timing without imposing
the provider ownership checks reserved for writes.

Only an actual absent selection produces the typed private
`CallbackOutput::StatusNoTab(Snapshot)`. The owner emits a narrow trusted reply
containing the actual catalog. The already-owned toolkit process strictly checks
that reply and invokes the complete original `statusWithTitle(undefined)` and
`profileStatus` closures. Both closure byte ranges and SHA-256 values are pinned
in `status-no-tab-native-manifest.json`; the full source digest remains
`e8b025a1c7de82ba8089c5fa7d4439ee69e1d19a74af7657a26a419a51cdfab6`.
Source revision 4340 is byte-identical to the admitted 4314 source. The existing
native toolkit validates the original upstream LICENSE through its installation
manifest; this increment does not copy or replace that license.

The original formatter is not reimplemented in Rust. Absent native observations
are never replaced by fake arrays, nulls, or a no-tab formatter for a present tab.
The original callback finalizer and actual mark-used accounting remain in place.

## Executed evidence (2026-11-08)

Three compile-only commands passed against the existing server target, with two
jobs, incremental compilation disabled, offline dependencies, the shared
exclusive lock, and filesystem space checks before each command:

1. Strict minimal `cargo clippy --locked --offline --no-default-features --all-targets -- -D warnings`.
2. Strict default `cargo clippy --locked --offline --all-targets -- -D warnings`.
3. Minimal `cargo test --locked --offline --no-default-features --lib --no-run --message-format=json`.

The compiled libtest digest is
`ae95254956f15185f340dc133c3b182d620623a250e903b96f9421a5f61e74c0`
(139,791,720 bytes). The production server and accepted profile-reporting example
remained unchanged. Compilation did not execute tests.

Runtime attempts are preserved separately; none was silently replayed:

1. Attempt 1 refused before container creation: Podman returned the exact image
   digest without the `sha256:` prefix. The strict string guard rejected that
   representation. No Node case or integration ran.
2. Attempt 2 executed all four Node formatter cases successfully, but Node's
   default `spec` reporter did not match the driver's TAP counter parser. The
   driver stopped before the factory integration. This was a failed harness
   attempt, not an accepted complete proof.
3. Attempt 3 selected TAP explicitly. All four Node cases passed, with zero
   failures, cancellations, skips or todos. The exact ignored integration
   `actual_toolkit_and_factory_format_absent_status_without_starting_page_worker`
   then passed once through the actual toolkit, service and factory. It covers
   implicit absence, an explicitly missing target, built-in and actual reported
   profiles, original finalizers, two mark-used receipts, clean accounting, and
   an empty Page-worker home. The desktop transport only enables the actual
   issued capability; it does not simulate a target or callback result.

All attempts used separate durable units and receipts. Accepted attempt 3 held
the shared exclusive lock until its CLI and exact container were terminal. Its
retained CID was
`ee9126de45024799caaf82bf1602c723bb111b5576b8d67b8b24e71dc1e57cec`;
the final exact lookup returned container-exists exit 1. Before and after the
accepted runtime, all 1,046 source inputs, all 26 files in the separate full
producer worktree, and all 5,162 fixed dependency/source/artifact inputs were
unchanged. No binary copies, hardlinks, new Cargo target, Page, Chromium or
Playwright runtime were created by this proof.

The machine-local receipts and their exact SHA-256 values are recorded in
[STATUS-NO-TAB-PROOF-4340.json](STATUS-NO-TAB-PROOF-4340.json). Receipt hashes bind
the executed source snapshot; this documentation update is subsequent to it.

## Executed negative and lifecycle proofs

Four actual cases now have qualified private evidence. They did not all pass in
one invocation or on one test binary:

| Cases | Actual evidence | Libtest SHA-256 |
| --- | --- | --- |
| Live/reserved-closing target refusal; held-ACK unready target refusal | First two cases passed in the initial negative sequence; that sequence later failed on the refresh assumption below | `1beb04ffdf59494e2898731dc6bc4801b154986250f9e878ea3f2ae8f1523e1f` |
| Explicit credential revocation; actual source-observer cancellation | Separate corrected sequence passed exactly two cases, once each | `43734b6a665ee02cb49d730a33e1b43ebfb81b5bcbda00532bb4c66564989783` |

The first physical case opens an actual Chromium target, verifies explicit and
implicit Status refusal, calls the actual `reserve_close`, and verifies both
refusals again while normal live selection excludes the closing target. No
`StatusNoTab` result or Page worker is created. The second holds an unaltered
actual attach acknowledgement through `native-status-cdp-gate.py`, proves Status
refuses during the real unadmitted setup, releases that same acknowledgement,
and completes that same setup successfully. No CDP result or target is invented.

The initial negative sequence stopped at case 3, after those two physical passes.
It incorrectly expected `Context.refresh()` to revoke the previous Context.
Actual source inspection and output showed that a provider-generation transition
can create a successor Context while the old read-only credential remains valid.
The assertion failed; case 4 was not run. The failed sequence and its logs remain
preserved, rather than being relabeled as a four-case pass. Production behavior
was not changed to force the expected error.

Only case 3 was corrected. Its positive control now exercises the actual old
caller after both the provider transition and refresh, proving read-only Status
still works. It then pauses a new actual callback, invokes the genuine
`Context.revoke()`, checks that the old caller's scope is revoked, and releases
the callback. The actual factory refuses, with no fabricated no-tab response.
Case 4 drops the actual owned Status invocation while its callback is paused.
The original source-end and observer-return receipts are retained; accounting
stays non-clean until the actual callback and finalizer complete. Releasing it
produces one actual no-tab callback result and one mark-used finalizer, then
clean accounting. Cancellation is not misrepresented as credential revocation.

The first two passing case bodies, shared fixture, and production logic were
byte-identical across the case-3 correction, so they were not repeated. Their
source body digests are included in the proof JSON. Both negative source
snapshots passed strict minimal/default all-targets compilation and minimal
libtest compilation before their reviewed runtime. The corrected two-case run
retained CID
`c85d3fc270524f210d8868cecace57f7423d3e39a8334080383a5c78cb575b81`
until exact container-exists returned 1. Its CLI exited 0. All 1,049 source
inputs, 26 separate producer WIP files and 5,411 fixed/dependency inputs were
unchanged before and after that run. The initial sequence likewise terminated
its exact container and preserved all of its 5,407 fixed inputs.

The timing wrapper delegates every callback result to the actual factory. All
four cases assert no Page-worker creation and real finalizer/accounting results.
The capability-only desktop transport is synthetic; target and CDP behavior in
the physical cases are actual Chromium. No installed desktop/Qt configuration
was modified. Machine-local receipts and exact hashes are in the proof JSON.
This documentation update follows the executed source snapshots.

Ordinary public CI for `b4f8acef527c4b0b9c4bdb2ae831cd9a0a9b94bf`
completed successfully in run `42465238296`. The library summaries were
478 passed/12 ignored with default features and 476 passed/12 ignored with
minimal features, both with zero failures. Its private native fixture tests were
ignored: ordinary CI does not substitute for the private proofs above. The
new negative tests were not part of that published CI head.

## Remaining scope

The public selector and bootstrap remain closed. Present tabs still refuse
Status. Complete present-tab Status and missing chooser, download, opener,
viewport and viewer producers remain pending. These bounded absence/refusal and
lifecycle proofs do not admit a complete Status formatter for an existing tab or
establish full browser parity. The separate full producer worktree remains
active and unchanged; this core slice does not import its unused lease or
read-wire APIs. No new build or runtime is authorized by this document.
