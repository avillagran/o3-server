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

## Remaining scope and proposed negative proofs

The public selector and bootstrap remain closed. Present tabs still refuse
Status. Missing chooser, download, opener, viewport and viewer producers remain
unproven. The separate full producer worktree remains active and unchanged;
this core slice does not import its unused lease or read-wire APIs.

The accepted integration does not prove refusal for live, closing or unready
targets, caller revocation, source cancellation, or complete present-tab Status.
Proposed follow-up tests, requiring a fresh source freeze and reviewed build:

- Open an actual target through the existing rootless CDP fixture, then call
  Status through the actual toolkit/factory. Assert explicit and implicit
  selections refuse without a `StatusNoTab` reply or Page-worker startup.
- Call the actual `reserve_close` on that target before Status. The raw registry
  entry must still refuse even though normal live selection excludes it.
- Hold the actual setup CDP acknowledgement after `open_owned` inserts its
  closing/unadmitted registry entry. Status must refuse while setup is pending;
  release the same acknowledgement and finish the same setup afterward.
- Pause a real service callback immediately before handing it to the actual
  factory, refresh the real Context to revoke its credential, then release it.
  The factory's fresh callback validation must refuse without a no-tab result.
  A provider ownership transition alone must retain the original read-only
  semantics rather than being mistaken for credential revocation.
- Drop the actual owned Status invocation while that callback is paused. Assert
  the original source-end receipt is retained, accounting stays non-clean until
  the callback/finalizer actually finishes, and no second response or worker is
  created. Cancellation alone must not be invented as a credential revocation.

These proposals have not been implemented or executed. They must use actual
registry entries and issued callers, not fabricated tabs or model-selected
absence. No new build or runtime is authorized by this document.
