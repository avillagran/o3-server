# Canonical local birth handoff: source checkpoint

This separate candidate starts at `a6595475ac035c07b0a7f43db1be0ea9adcffbb6`
and includes the independently accepted five-file early-dialog correction.
The accepted worktree and the original 26-file producer worktree are unchanged.
Strict minimal and default metadata-only all-targets checks passed for the
installer/handshake checkpoint `e76d9f4`. Source checkpoint `833ffc2` subsequently
passed a normal minimal library build and three scoped physical cases: one
positive birth-order case and two cancellation cases. The earlier strict
metadata result does not cover `833ffc2`. These physical runs used the separately
admitted 4314 native runtime, not the later 4367 native source. No existing
application/test executable was replaced or libtest reservation released.
Public present Status and its bootstrap remain closed.

## Actual source dataflow

`Browsers::open_owned` now stages a private `BirthLease`. It owns the same new
`about:blank` target, registered Page session, closing registry entry and setup
mutex guard. Author navigation and public admission are separate publication
steps. Public `page()` and `capture_native_page()` continue to refuse the closing
target. The shared internal capture routine accepts only a real owner-created
Page handle; the birth path creates that handle from the sealed lease, not JSON.

A real `OwnedPageFactory` registers a weak trusted birth installer. It cannot be
selected by an MCP parameter. The first installed coordinator is not silently
replaced by another facade; a configured coordinator that retires refuses later
births instead of falling back to an unobserved legacy path. Without any trusted
installation, existing preview callers retain their legacy behavior and obtain
no producer certificate or Status authority.

For a new local birth, the installer starts/reuses the exact physical slot with
`StartPurpose::LocalBirth`, obtains its real worker initialization result and
requires a typed source-issued producer binding receipt. The owner writes the
fixed `canonicalBirth` configuration from the actual target identity. Existing
or desktop/adopted Page attachment does not receive that configuration and does
not install these birth-only bindings.

The canonical worker installs actual request/load/main-frame/request-failure,
dialog, chooser, download, popup, close and crash listeners. Its private bounded
ledger retains actual Playwright objects; it is not a Status projection. It
starts with explicitly unobserved initial history, and overflow stays unknown.
For the new local Page, original `updateFileChooserInterception` always enables
interception (`!tab.desktop`). The installer additionally waits for the genuine
fixed `Page.setInterceptFileChooserDialog` acknowledgement before issuing its
binding receipt, instead of equating the asynchronous Playwright listener hook
with completed interception.

The worker's framed ready acknowledgement includes the exact physical target
and exact installed event list, under its existing private nonce. Rust validates
the whole envelope and retains an opaque typed `ProducerBindings`. A legacy bare
ready packet remains valid for existing worker uses but cannot satisfy a birth
installation. The lease checks the same NativePage allocation, target, live
process/session/registry entry and source receipt before releasing its retained
author navigation. It retains the actual trusted installer and installation
through navigation/publication. A per-physical-slot retirement read guard keeps
the canonical observer task alive until that publication finishes or aborts;
factory shutdown takes the corresponding write guard before retiring the worker.
The installer closing state is revalidated after its actual acknowledgement. Missing/mismatched acknowledgement retires the actual worker;
unknown retirement is reported as `birth_cleanup_unknown`, never ordinary empty
Status. No Control/RunContext is fabricated and generic action write admission
is unchanged.

## Remaining gaps and required proof

This code establishes the installation handoff and positive event collection;
it does not implement the complete original producer semantics or formatter.
The event list is installation evidence, not an absence certificate. In
particular:

- Audit the exact scriptless blank birth and initialization sequence; no event
  silence, URL equality or generic worker-ready Boolean establishes absence.
- Port full original chooser current-document/accept/replacement/controller
  closures; a retained chooser object alone is insufficient.
- Implement original completed download save/stat/history/eviction through owned
  artifact boundaries. The existing native deny policy remains; a retained
  download object is not a saved download or an empty-history certificate.
- Adopt actual popups under separately issued physical grants and preserve
  opener/profile/thread identity. A popup object alone is not completed adoption.
- Bind real viewer/control/catalog and acknowledged resize/zoom/desktop viewport
  facts. Those fields are not initialized by this receipt.
- Connect these facts to actual private Status consumers and execute the full
  admitted original formatter; no fake arrays/null fields or dropped candidates.
- Extend the accepted delayed-ACK and target-close cases to wrong target/nonce,
  partial installation, caller cancellation, retired installer and unknown
  cleanup. Confirm interception and observer cleanup after worker retirement
  without granting model writes; the three accepted cases do not cover all
  failure and replacement paths.
- Review first-coordinator retention and explicit replacement after clean
  retirement. Public installation/bootstrap remains unavailable.

The bounded ledger is deliberately not wired as completed Status output. Missing
field producers still refuse the full operation. No `allow(dead_code)`, public
selector opening or existing admission visibility widening is used to mask a
missing consumer. The new internal module visibility only joins its sealed
receipt to its actual lease/factory consumers.

## Actual metadata gate

Source checkpoint `e76d9f4aa30dc486c1f137b3203986c9cc7e3efa`, tree
`e7135b9d054bf5795ede11f9d5085721839a6f8b`, passed exactly:

- `cargo clippy --locked --offline --no-default-features --all-targets --message-format=json -- -D warnings`
- `cargo clippy --locked --offline --all-targets --message-format=json -- -D warnings`

The private rootless job used the existing target, shared exclusive lock, two
jobs, incremental compilation disabled, offline dependencies, and the original
root/source/target floor. Its result is
`14baecdfbe47e7ea6d7345e751034c9df0558d1fff4fb4bc775ee7314d392d8e`.
Minimal and default commands completed in 30.82 and 38.04 seconds respectively.
The two complete emitted graphs are
`5fa020834cdd16f5c53e8b6ebd3092f44a534338eadcc4e90e834758365c8839`
and `0bced2ce2afe540c31bb900591283b415d01dbe8762dee4cbea4026904d79145`.
All 67,368 admitted inputs, the exact 14 candidate files, original 26-file
worktree and 24 executable aliases remained unchanged. The reserved libtest
`1a382055b7d7e3d57c0dcd781e84bf68084b29dc898d6d9ac2bc259c199d3fa9`
and managed-catalog probe remained read-only. The actual container name and
retained CID were terminal, with no deferred signals or ownership audit errors.
This is type/lint validation only; zero tests ran and no complete Status or
physical birth-order acceptance follows from it.

## Error-retirement lock graph

The installation holds the per-slot retirement **read** guard. Its direct error
cleanup calls `ControlledPage::retire_worker`, which sends the independent
Worker stop signal and awaits the Worker supervisor's own close receipt. That
path never takes the slot's retirement **write** guard or waits for `Slot.closed`.
The supervisor reaps the child, closes the native gateway and publishes its own
receipt without acquiring `Browsers.setup` or the slot gate.

The background slot retirement takes the **write** guard, calls the same
idempotent worker retirement and then publishes `Slot.closed`. Concurrent factory
shutdown waits for that slot receipt. An erroring installation therefore first
gets the independent Worker receipt, then releases its read guard on return;
the background writer can proceed and read the already completed Worker receipt.
The captured registry mutex is dropped before these awaits. This source graph
has no read-to-write self-wait. The physical cases below cover target closure
before entry and after command entry. Mismatch and other retirement races remain
separate proof obligations.

## Per-target cancellation phase

Each actual new local target now gets a distinct `TabActivity` allocation and a
typed setup phase: staged, navigation entered, canceled before entry, canceled
after entry, or admitted. Legacy/adopted activity remains separate. This phase
is cancellation/admission identity only; it cannot certify producer readiness,
initial absence or any Status field. Existing admitted/created/used facts remain
unchanged except when a real birth is successfully admitted.

`closing_tabs` cancels the staged phase under the actual target registry lock.
The fixed author `Page.navigate` uses the existing exact actor writer and an
owner-only `write::Authority`. At `Permit::begin`, that authority takes the same
registry lock, checks the actual activity Arc, actor channel, target and session,
and atomically changes staged to navigation entered. Cancellation before this
point sends no author navigation. Cancellation afterward marks the entered birth
canceled but does not abandon its original command wait; its genuine reply still
settles the owned operation. Publication rechecks the same identity and performs
the final phase transition under that registry lock. A canceled phase cannot
set the admitted flag or publish an available tab after settlement.

Error cleanup retires the canonical sessions before raw target rollback and
preserves `birth_cleanup_unknown` on incomplete retirement. The publication path
retains the installer, installation and per-slot retirement guard throughout.
`current()` also checks the actual activity Arc, in addition to actor/target/
session identity. The new registry-to-Control snapshot order has no inverse
callback path in the current source: Control's entry predicate uses atomics,
and the driver releases its state mutex before polling Page callbacks. Browser
close also releases the tab registry before awaiting Control shutdown.

The public-API physical probe uses a real canonical worker framed-ready proxy
and a real Chromium pipe navigation witness. Its positive case holds the
original producer ACK before the first HTTP navigation. Its two negative cases
close a real pending tab before ready release, and close after a real navigate
frame enters while its original ACK is held. All three scoped cases passed as
recorded below; they do not establish complete Status or initial absence.

Source checkpoint `fb836cd` was refused during source review before Cargo: an
incomplete edit left the rollback helper/signature missing. Its immutable source
receipt is retained. The next source checkpoint completes that helper, including
canonical retirement on the early invalid-current path; the earlier metadata
PASS is not attributed to this later phase increment. The physical fixture now
also retains real enabled chooser-interception responses on source-derived
owned sessions, before its separate genuine worker-ready frame barrier.

## Accepted physical evidence for source5

The exact tested source is checkpoint
`833ffc2352ca18bab57663d7d54c8e90d156d466`, tree
`a9b7bcbb10279df5f92b287748527b2c84d429b4`, with source manifest
`c0fb7be31933a84a9e87bce6bdc4622b0c3cf8b3ca6365b3072ac7f68b445437`.
The documentation update containing this section changes no tested code.

The positive job first ran normal `cargo build --lib --locked --offline
--no-default-features --message-format=json` in its private container. It selected
the actual root package and the three public externs from the emitted graph,
admitted native link files through descriptors, and linked the unchanged
public-API probe only into private RAM. Its root library hash was
`fd9f60ad92106e39fee97f0cabc2d42259ccd0121b77eb1405b1d9bad10bc234`.
The complete library graph hash is
`98c97da6cb33bfa2dc642fb33da78d30430e5c9de615634d149e9adaacd21c15`;
the selected-four-extern admission hash is
`cd47c0e9bd09fe0433ba599eda25c30e5984cbbecf5607ba362ef81b03f2361b`;
the native-link admission hash is
`99361b989ecd47bfd4437f4ffccac6ca78533fdff1fe43312d42349a3965c120`.

| Gate | Actual result | Immutable result receipt SHA-256 |
| --- | --- | --- |
| Producer acknowledgement before first author HTTP navigation | 1 passed | `f7c2766d969f4c1a172a68bb353b36914186909d209813abe8be9b82814bdf47` |
| Close while producer-ready frame is held; no author navigation or admission | 1 passed | `8f0ad146c800c678cd3f14cda93b56691941da5ec8884799012b0b51abd99631` |
| Close after actual navigation entry; original ACK settles before refusal | 1 passed | `8f0ad146c800c678cd3f14cda93b56691941da5ec8884799012b0b51abd99631` |

The positive result is retained under
`/var/tmp/o3-server/status-canonical-birth-physical-f7qIOreC/result.json`.
It held and released two genuine enabled chooser-interception CDP responses,
then the unchanged 325-byte producer-ready frame, before the first actual HTTP
navigation. The same physical target was admitted. All 71,765 admitted inputs,
source5, the original 26-file worktree, and 24 executable reservations remained
unchanged.

The two-negative result is retained under
`/var/tmp/o3-server/status-canonical-birth-negatives-_5atk59q/result.json`.
It used that same actual library/native graph with the entire existing target
read-only: no Cargo, root-library rebuild, positive repeat, or existing binary
replacement. An identical probe was linked only into fresh private RAM. Each
case owned a fresh private fixture and genuine packets. The before-entry case
observed no author navigation and zero HTTP hits after closure; the after-entry
case held the original real navigation response, observed the opening operation
still pending after closure, released that exact response, and refused admission
after settlement. Exactly two cases passed, with 17 complete raw exports and all
71,808 admitted inputs unchanged.

Both jobs retained the same actual client, container name and exact CID through
terminal state under the shared exclusive lock. Both names and retained CIDs
were absent at completion, with zero deferred signals and ownership audit
errors. The original three-second fixture barriers and product deadlines were
unchanged. No synthetic CDP event/reply or installer callback supplied positive
facts, and no model write authority was introduced.

These receipts qualify the isolated source5 implementation against its admitted
4314 native runtime only. The combined primary source, 4367 manifest and typed
full-resolution screenshot decode require fresh combined metadata/library graph
and all three physical cases against the staged 84-file 4367-equivalent native
source before combined acceptance. Full Status field producers/consumers,
initial absence, and public installation/selector activation remain pending.

## Combined strict phase1: retained failure and source correction

The first combined-source compile job stopped at minimal strict all-targets
metadata. The current admitted Rust 1.99.1 compiler rejects two deprecated
`AtomicU8::fetch_update` calls under `-D warnings` and recommends `try_update`.
The result receipt is
`ccf157d6dd675a3091913eaf982e032148248f69aa13c1627022289d07cfaea1`,
retained under
`/var/tmp/o3-server/status-birth-combined-validation-89atpwjy/result.json`.
The complete minimal JSON graph is
`320f7cd397c908db9ee07d65c30fc5a15618409c157881ea434d7bff2a9f6e80`.
Default metadata, normal library build, and runtime were not reached. All 71,774
admitted inputs and 24 executable reservations remained unchanged; the actual
client, retained CID, and container name were terminal. This is a failed strict
check, not a combined success or physical test failure.

The next source correction renames exactly those two methods to `try_update`.
Their closures and success/failure memory orderings are unchanged. No warning
allowance, producer authority or readiness behavior changes. The original
checkpoints and physical evidence remain immutable and qualify their original
source only. Fresh combined strict metadata, actual normal library graph and
three physical cases remain required for the corrected combined source.

## Combined 4367 validation and formatting

The corrected combined source at checkpoint
`429dd1f8828b6ca5dcd7d1dbc5dce9299a0e720a` passed both strict metadata modes
and a normal minimal library build. Its retained result is
`9ce0be39959ee70896eda626a43a242a18ab52cddc9341028c90aa6ed9c0005b`,
under `/var/tmp/o3-server/status-birth-combined-compile2-8rokyle_/result.json`.
The failed first generation above remains preserved; it was not rerun unchanged.
The only production correction before generation2 was the two-site atomic API
rename, with identical closures and memory orderings. All 24 reserved executable
identities and hashes were compared to generation1 and remained unchanged.

The three compilation commands were the same strict minimal/default all-targets
Clippy commands listed above, followed by normal minimal `cargo build --lib`.
A minimal `cargo metadata` query resolved the actual graph. No tests, production
binary link, libtest replacement, custom SDK graph, or new Cargo target was used.
The complete emitted graph hashes are:

| Graph | SHA-256 |
| --- | --- |
| Minimal strict metadata | `f720331de4b437a251cbd51eb83e67ed469041e24f346e430d475a973c3f4faa` |
| Default strict metadata | `35f1d4e8b2403bd42f501399e9e0f39e6ee70bd00f7bc9499be9d36840263ccf` |
| Resolved metadata | `c56122d24616ef6b90e1e53f429d55302a726650f950ae812243dd8b99fccadc` |
| Normal minimal library artifacts | `3fc9dbf8f7fc465342be906e12639f93b3a960efa7c67db0d6d54bd268965a13` |
| Exact four public externs | `2eb63edc7cd9343f79786cf2ff356724bcfa39ea180e10ceee191fccb3831768` |
| Actual native link paths/files | `8d3abec8e0b178032affd6f1d83972890f23d63fda9b2e9c65f9fd9a9fe9621c` |

The actual selected root library hash was
`e3bf5a9f439e2095eb93de8ecf0d5bc6c1894886c387b39b7e8b3894dd9b6208`.
Its package, profile, manifest and source identities came from the emitted
normal graph. The selected four externs and all 35 native files, their directory
memberships, descriptor identities and hashes were independently checked before
runtime admission. All 71,791 compile inputs remained unchanged at completion.

The separate combined physical job then ran the unchanged public-API probe
against the staged 84-file native source from 4353, independently verified
identical to 4367, with the preserved 4367 manifest and typed full-resolution
screenshot decode. Its native source manifest hash is
`a557d15024907934f6b48e83871bff88eb3483a538e8db32d005b13428a1374b`;
the product native manifest hash remains
`7aa24a729358302b9e915fe96f62ee4e2dd23df88c589e66db91d060b187ea9d`.
Exactly these three cases passed, each in a fresh private fixture:

- Genuine chooser and producer-ready acknowledgement before the first author
  HTTP navigation, followed by admission of that same physical target.
- Close while the genuine producer-ready frame is held, then release that same
  frame: no author navigation, zero HTTP requests, and no admission.
- Close after genuine navigation entry while its original response is held:
  the opening operation remains owned until that exact response settles, then
  refuses admission.

The result is
`646c39805c9e5bd5f7013f3d81d3457a8e1bba8194ef495bfd251d5d92f8228c`,
retained under
`/var/tmp/o3-server/status-birth-combined-runtime-5um93pby/result.json`.
Exactly three cases passed with 25 complete raw marker/stdout/stderr exports.
There was no Cargo invocation or library rebuild: the entire existing target
was read-only, and the unchanged probe was linked only into private RAM using
that actual graph. All 71,838 admitted inputs and all 24 executable reservations
remained unchanged. The same actual client, exact container name and retained
CID were observed to terminal, with zero deferred signals or ownership audit
errors. The three-second fixture barriers and product deadlines were unchanged.

A subsequent container-only admitted rustfmt check found one line-wrap and
indentation difference in `cancel_birth`. Normal edition2021 rustfmt changed
only `src/server_browser.rs`; its non-whitespace bytes are identical to the
physically tested preformat source. The formatter-only primary checkpoint is
`206207c92ecf87ffcda2bba2dc6ac18656ca7f13`; the preformat checkpoint and all
physical receipts remain preserved. The source file hash changed from
`25699b4e03547c76d3aefaa27fc4de37b6c6b00b8d81c07b71eef0cac4c88ada` to
`80285ac9b70b998c17a366a282950c5a2868237b4e9600afa212e141e559c250`.
The fresh static check of exactly the six changed Rust files passed with
`--edition=2021 --check --config skip_children=true`; its result is
`13b43a322eef51eff3a53585fdd1a5166544dde7a63e6d485dbee3965242cec8`,
under `/var/tmp/o3-server/status-birth-format-check2-p1l1gazd/result.json`.
This check executed no tests or Cargo build and preserved all 24 executables.
No semantic change or repeat of the physical tests was needed for formatting.

These are scoped combined birth-handoff proofs, not complete Status acceptance.
The six existing full-resolution capture component cases also passed against
the materially changed combined worker and its birth-module import. That
separate JS-only runner froze the exact worker, birth module, native source,
actual imports and dependencies before this evidence-only documentation update.
Its result is
`597973282b12c7364f4c3b35225a5b64527d1ff1c7a977037e0ec8d13147798e`,
under `/var/tmp/o3-server/preview-full-capture-combined-if2s3rxj/run1/result.json`.
The unchanged six-case probe verified invalid native input refusal before CDP,
a 2048-pixel full image alongside its 1280-pixel capped image with pixels
preserved, omitted and explicit-false capped-only behavior, no duplicate image
for an uncapped viewport, and actual DPR2 producing a 2560-pixel full image
alongside the 1280-pixel capped image. Real scoped Chromium and the whole native
Page capture function were used. Chromium exited successfully, all 2,582 pinned
inputs and 24 executable reservations remained unchanged, and the same client,
name and retained CID were terminal with zero deferred signals or observation
errors. Actual recursive artifact requests were quarantined; no recursive
removal was executed. No Cargo, target mount or runtime copy was used.

Capture uses explicit empty diagnostic fixture inputs and cannot certify Status
absence. These six capture cases do not activate the full router, MCP save/image
projection or public selector. New-head normal CI remains required after
publication, including its whole-repository formatting check and both
functional test modes.
