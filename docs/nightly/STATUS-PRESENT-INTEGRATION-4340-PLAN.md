# Present-tab Status integration: required producers and next source slice

This bounded implementation starts at public commit
`6be0e55fdc0f16ca67599c40b591bc57371fef8b`. The separate 26-file producer worktree
is preserved, not cherry-picked. Strict metadata checks, a minimal libtest build,
and the two scoped private runtime cases passed. Complete present-tab Status and
the public selector remain pending.

## Bounded production correction

`Observer::accept` previously ignored an actual `Page.javascriptDialogOpening`
from its registered opaque session until `main_frame` was populated. Registration
already occurs before `Page.enable` and `Page.getFrameTree`, so an opening while
frame discovery is pending could be lost permanently. The event belongs to the
exact registered Page session; a child-frame dialog also blocks that Page. The
prepared correction removes that local frame-discovery prerequisite, preserving
the existing exact-session check, UTF-16 packet handling and detach retirement.
The existing regression now requires acceptance before frame discovery and
continues to reject an unregistered session and discard detached state.

The admitted Playwright 1.60 protocol type at
`package/types/protocol.d.ts:15186` declares `frameId` as required, and
`defaultPrompt` as optional. It does not declare `frameId` optional. The pinned
Chromium `PageHandler::DidRunJavaScriptDialog` and `DidRunBeforeUnloadConfirm`
pass the actual frame token into the opening event. Neither fact justifies
waiting for an independently cached main-frame tree. This slice does not add a
frame-ID requirement that could discard older protocol events.

Local launch adds exactly these three switches retained by the original
ServerBrowserContexts Playwright defaults:

- `--disable-component-extensions-with-background-pages`
- `--disable-default-apps`
- `--disable-extensions`

The complete original ServerBrowserContexts source excludes only
`--enable-automation` from defaults. The admitted Playwright `coreBundle.js`
contains the three switches at lines 33631, 33635 and 33639, and its default-args
filter at 38321 retains them. This is launch alignment, not an initial-dialog
absence certificate. No installed Qt or desktop configuration is modified.

## Actual held-frame-tree acknowledgement proof

Use the already admitted rootless actual Chromium pipe proxy pattern, with a
new private fixture gate. Do not synthesize a CDP event or reply.

1. Register the actual Page observer before enabling its domain, as production
   already does. Retain the exact real `Page.getFrameTree` response in the proxy.
2. Through a fixture-owned, fixed command path, ask that exact Chromium session
   to execute a literal alert only after the held request is observed. Forward
   its genuine opening event immediately. Record the original frame response,
   the event and their actual IDs. No model expression or new production method
   is admitted. If an earlier real frame-navigation event would populate the
   cache, explicitly retain that original packet too; assert the intended
   unknown-frame precondition rather than claiming it from timing.
3. While the original response remains held, assert the production observer has
   the real pending dialog, matching its message/default UTF-16 units, with
   `main_frame` still unknown. Foreign-session traffic must not set or clear it.
4. Resolve through an actual acknowledged dialog operation, release the same
   held native response, and finish the same setup. Assert no duplicate target,
   replayed event, extra navigation or automatic dismissal was introduced.
5. Retain source/Chromium/dependency pins, the exact container CID and actual
   terminal receipts. Run the isolated regression and actual race case only
   after a fresh source freeze and Root review.

Source for this actual proof is implemented and privately validated in
`tests/fixtures/input/native-dialog-frame-gate.py` and the ignored
`actual_dialog_opening_is_retained_while_original_frame_tree_ack_is_held` test.
The fixture binds its session from the actual `Page.enable` request, holds real
same-session frame-navigation packets and the first actual frame-tree response,
and injects only the fixed literal alert after that response is captured. This
is an explicit fixture delivery gate, not a claim about natural Chromium event
ordering. Every held original packet is released once without changing its ID
or payload. The test asserts the observer's unknown-frame precondition directly.

The fixture's request ID is -1; native pending request IDs start at positive 1
and use u64. The pinned Chromium session dispatch uses signed integer call IDs.
The proxy refuses any nonpositive native ID and consumes only its own -1 reply,
so it neither rewrites native IDs nor forwards its injected response into native
pending accounting. Its real evaluation reply must arrive without a CDP error.
The actual private Chromium run accepted this negative request ID and returned
a successful evaluation response; this is evidence for the pinned runtime.

The gate is capped at three seconds, below the existing five-second native
command deadline. Expiry fails the fixture and terminates/reaps its actual
Chromium child; it never fabricates a response or automatically claims success.
All post-Popen socket setup is inside the child-ownership try/finally, with
optional socket handles guarded and the real child always waited/reaped. CDP
packets and unterminated buffers are limited to four MiB, matching the native
pipe admission; held original packets additionally cap at four MiB/eight packets.
The Rust proof has bounded waits, answers via actual pending-dialog token and
acknowledged Page.handleJavaScriptDialog, releases the same frame reply, verifies
one target and the same physical identity, and checks its evaluation terminal.
Marker JSON is published atomically through exclusive no-follow temporary
regular leaves in the owned private directory. No fixture tree is deleted.
Foreign-session rejection remains covered by the separately prepared observer
regression; the actual race fixture does not forge foreign CDP traffic.

This proves preservation of a positive opening event during discovery. It does
not prove absence before observation, all-script pause, Page attachment safety,
or complete Status fields.

## Scoped validation and retained receipts

The five-file source was frozen before validation as
`cc2a4d8ec41de06cd94ac0d4bf8f5c0b74bff728b1aa963ba59b0d384571e7f2`.
The two Rust files and actual pipe fixture remain byte-identical to that freeze;
only these documentation/provenance records were updated afterward.

The first compile attempt passed minimal strict all-targets metadata checks, then
failed during default checks because an overly broad executable-mode inventory
mounted a genuine Cargo shared-library cache output read-only. It never reached
the libtest build or runtime. Its retained result is
`4d516ff63455f842983c301d89bb0b0a24b62beb8413d926c3198a49bed0297f`.
The corrected classification permits normal Cargo shared-library cache rebuilds,
including proc macros and cdylibs, while preserving all actual application,
probe, example and unrelated test executables.

The fresh corrected compile passed these exact commands inside the reviewed
private rootless container, using the existing server target, two jobs,
incremental compilation disabled, offline dependencies and the shared exclusive
lock:

- `cargo clippy --locked --offline --no-default-features --all-targets --message-format=json -- -D warnings`
- `cargo clippy --locked --offline --all-targets --message-format=json -- -D warnings`
- `cargo test --locked --offline --no-default-features --lib --no-run --message-format=json`

Its result is
`d1f39e1fc62bd8d61e3ced5d0423e5bcd8049a86fa2e5aaecc02a729429ef684`.
Normal declared Cargo dev dependencies emitted the SDK rlib and static-library
side output; no custom SDK graph or manifest rewrite was used. Only the prior
minimal libtest reservation was explicitly released for the third command.
The new 138,585,616-byte libtest is
`1a382055b7d7e3d57c0dcd781e84bf68084b29dc898d6d9ac2bc259c199d3fa9`.
Production, profile-reporting and managed-catalog probe executables and their
aliases remained protected. The compile step executed no tests.

The separate private runtime then passed exactly one observer regression and
one ignored actual Chromium held-frame-ACK case. The runtime result is
`8ff1479a6468270980c9a9baa48a307de236de660e8f429062ebcb7e366ef2de`;
the complete log is
`424551bdfda25fbc405807494239865165309f6275523f3eda79b2ea20d9677c`.
All five genuine fixture marker/stderr artifacts were exported with complete
hashes. The same actual frame-tree response was held and released once; the
owned observer recorded the real opening while its cached main frame was
unknown, and the injected evaluation completed successfully. Both the exact
container name and retained CID were observed terminal. All 67,600 admitted
runtime inputs, 1,052 source files, the original 26-file worktree and protected
artifacts remained unchanged; there were no deferred signals or ownership audit
errors. No fixture tree was recursively deleted.

This accepts positive event retention and the bounded launch alignment only.
It does not establish initial dialog absence, canonical-worker readiness before
author navigation, complete present-tab producers or public Status admission.
These receipts are distinct from ordinary CI, whose ignored physical proof does
not run automatically. Reference 4353 changes the screenshot pipeline; these
4340 source pins are not a full-file equivalence claim for that later revision.

## Full formatter field and producer matrix

The full original source remains SHA-256
`e8b025a1c7de82ba8089c5fa7d4439ee69e1d19a74af7657a26a419a51cdfab6`.
`statusWithTitle` must be executed as the complete hash-admitted original closure,
with `profileStatus`, `tabOwner`, `dialogStatus`, `layoutSize` and their complete
transitive dependencies. The original formatter is not rewritten in Rust.

| Original field or branch | Actual producer required | Current gap in the preserved worktree |
| --- | --- | --- |
| Selected tab, `tabId`, `available` | Actual thread/explicit selection, physical identity and live registry | No present-tab coordinator; source-only no-tab path is already qualified |
| `profiles`, optional default | Actual catalog reports and original profile closure | Qualified no-tab catalog path can be reused |
| URL and candidate-tab URLs | Actual bound frame/navigation/target events and original data-payload redaction | Selected URL observed; complete candidate enumeration/freshness still needed |
| `visible`, candidate visibility | Actual viewer registry matching original `tab.viewers`, not desktop-shown state | Native viewer registry exists; exact original viewer equivalence is not proven |
| `loading` | Continuous actual main-document request/load transitions from birth | Native navigation exists; adopted initial loading remains unknown and needs source equivalence |
| Control owner/caller/generation | Actual Control snapshot and original `tabOwner` closure | Actual snapshot exists; never request or manufacture a write RunContext for Status |
| `dialog` and title skip | Continuous actual dialog objects/events from a proven birth | Initial state is correctly Unobserved; pre-frame event gap is corrected in this proposed slice |
| `fileChooser` | Original interception policy, actual chooser event/object, acceptance/current-document checks and close/navigation lifecycle | No complete producer; extracted helper is missing transitive `fileChooserMessage` |
| `viewportSetting` | Actual initial setting and acknowledged subsequent resize/zoom updates | Birth setting only; not a current resize acknowledgement |
| Local `viewport` | Canonical Page viewport and original non-desktop branch | Current canonical Page readiness/configuration is not a complete producer |
| Desktop viewport | Complete original serialized isolated-world read, last usable size and actual pending-dialog behavior | Fixed read wire exists but no actual coordinator consumer; desktop birth remains unproven |
| Candidate `openerTabId`/profile | Actual target birth/adoption and manager profile snapshot | Profile birth exists; opener and popup adoption are absent |
| `downloads` | Original actual download completion, saved file/stat, limits, history and disposal | Local actor currently denies downloads; empty history cannot replace the original producer |
| `title` | Canonical `Page.title()` through the typed fixed read, or the original explicit blank/dialog skip | Typed private wire exists; no actual consumer; unknown readiness must refuse |

An original formatter returning null, an omitted viewport, or an empty download
list after a genuinely observed branch is different from using those values to
hide missing producers. Missing required observations refuse the whole Status.
A missing candidate URL/opener/profile observation cannot be hidden by dropping
that candidate from `tabs`.

## Birth and canonical Page ordering required before complete consumption

The local owned path creates a new `about:blank` target without an opener or
preload over its private inherited pipe, registers/enables the raw observer and
then navigates. However, it currently performs the author navigation before a
canonical Playwright worker/listener is ready. Connecting Playwright afterward
cannot repair missing chooser/download/dialog history, and may introduce a
listener-initialization gap. Desktop adoption has an additional unobserved
history gap. `Page.enable` acknowledgement/no event does not prove absence;
`waitingForDebugger=true` does not prove no earlier script execution.

The necessary next product source slice is a private owner birth/readiness
handoff, followed by real producers:

1. Retain exact new target/process/connection identity and keep admission closed.
   Establish an audited scriptless birth for that exact new empty target, with
   original launch defaults and no opener/preload/init-script bypass. Do not
   borrow an initial singleton page or URL-matched target as a birth receipt.
2. Install the continuous raw observer and the canonical Page's real dialog,
   chooser, download and lifecycle listeners before any author navigation. The
   actual canonical worker readiness acknowledgement must precede release of
   the existing navigation, not be bootstrapped by a Status callback.
3. Only a source-audited physical birth and continuous observer coverage can
   initialize genuinely empty observations. Adopted or gap-containing pages
   remain Unobserved and refuse. No public Boolean, model input, generation seal
   or Page.enable silence may initialize them.
4. Port complete original chooser/download/popup helpers with all transitive
   closures, actual objects and owned artifact boundaries. Producer setup and
   event reads need their own fixed trusted scope where necessary; do not put
   arbitrary element evaluation, file writes or chooser answers under Title's
   Status scope. Answer operations retain the generic write guard.
5. Update resize/zoom/viewer/control/catalog facts from their actual owners and
   preserve per-thread candidate identity. Every required field must have an
   actual producer before the present selector opens.

The old Qt adoption path has no continuous scriptless birth observation. It
cannot inherit a clear-dialog or empty-history certificate. Its native owner
factory/protocol migration needs independent source review; this worktree does
not modify the installed Qt application.

## Consuming the preserved private read APIs without weakening authority

After the complete required producer state is real, the actual factory can add a
private present-tab StatusCoordinator/SourceExecutor callsite:

- Enter the one-use source Status callback and revalidate the actual caller.
  Preserve the original read-only provider-transition behavior and genuine
  revocation checks already proved by the published tests.
- Select the actual existing tab without the action-only multiple-owned or
  agent-write policy. Require an already-ready canonical worker; never call the
  worker-creating `slot()` fallback from Status.
- Bind `EnteredStatusCallback::bind_page` to the exact current owner. Use
  `StatusObservationLease::begin_read`, `StatusReadReceipt::into_invocation` and
  `Worker::invoke_status` as real callsites only when the complete original
  formatter actually requests title or desktop viewport. Add the narrow
  ControlledPage bridge; no Control/RunContext/write/generation admission is
  introduced for these reads.
- Run the complete original formatter with actual selected/candidate facts and
  canonical Page reads, preserving JavaScript code units and original error,
  timeout, last-value and null/omission branches. A known completed empty result
  is not an Unobserved producer. Do not replace original branches with a global
  `layoutObserved` Boolean.
- Preserve queued versus started frame accounting, actual late terminal/end
  markers, cancellation, source-end/finalizer receipts and sticky unknown
  outcomes. Before publication recheck identity, actual caller and required
  producer coverage. Unknown cleanup never becomes an ordinary empty result.

The title/viewport wire in the separate worktree has no genuine production
consumer yet. Importing it before a real callsite would leave unused private
entry APIs; neither visibility widening nor `allow(dead_code)` is acceptable.
Import the dependency-complete slice together with its real private consumer,
not the whole dirty worktree or unrelated event code blindly.

## Complete present-Status proof gate

A later reviewed proof must exercise a real owned new target and actual original
formatter through the whole source callback: initial blank and author page;
agent/unclaimed/human ownership and multiple-owned selection; actual viewer
attach/detach; main/same-document/failed navigation; prompt/confirm/beforeunload
and close races; chooser replacement/navigation; actual completed/failed/evicted
downloads; popup/opener/thread isolation; local resize/zoom and desktop last-value
viewport; UTF-16 title/dialog text; provider transition, explicit revocation,
observer cancellation, late frames and unknown terminal outcomes. Required
unknown observations must refuse, including adopted/legacy targets with gaps.
Compare original whole-closure output and actual side effects, not a hand-built
expected projection alone. Only then can full present Status and its public
selector be considered for admission. The full parity goal remains active.
