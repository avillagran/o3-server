# Owned native Page factory (source checkpoint)

This work targets nightly 4314, commit
`83a3674ffd84ffb33de768210c806cb1ac53fd80`. It is not a published MCP catalog or
an acceptance claim for all twelve preview tools. The private Rust factory and source admission compile with strict minimal-library
Clippy. Focused owner/control tests pass as described below. The first physical evaluation slice passes with its explicit status limitation
below; broader factory acceptance remains pending. No public MCP catalog is installed.

## Physical Page and worker ownership

`PageInstallation` is a trusted Rust startup API. The supplied `WorkerSpec` binds
the admitted Node, worker entry and native/Playwright/Effect installation paths;
its private home is an existing owner-only parent for separate Page homes.
There is no wire decoder for this installation and no ambient endpoint lookup.
The factory creates private homes through retained directory descriptors and
never removes their trees.

`OwnedPageFactory` attaches only an existing actual browser tab. It captures the
legacy physical transport and `Manager.capture_native_page`, verifies their
actual Control identity/current transport, obtains that Page's private gateway,
seals its receipts with `configure_worker`, and starts the real `Worker`.
Each cache slot uses `NativePage.same_owner`, which compares the actual owner
allocation containing its registry and target. A credential, tab label or
ordinary Control generation cannot create a second canonical Page/Keyboard.
One factory must be shared across toolkit callers for an application State.

Setup has its own owned task and terminal watches. Dropping the first caller's
observer does not abandon setup, manufacture readiness or restart a failed
worker. A clean retired owner may be captured anew under the existing native
grant rules; unknown retirement remains a refusal.

## Source callback entry

The factory consumes only the source service's privately constructed
`SourceCallback`. Its stored request, caller, raw JavaScript input and original
tool class remain immutable. The source callback mints a `NativeInvocation`
inside the actual captured Control's `RunContext` and retains the actual worker
execution through its terminal receipt.

The new `source_agent_admitted` path submits the usual ownership/generation
guard to the existing kernel with no floating-point deadline. On `Event::Run`,
under the scheduler lock, it checks the captured exact Tokio instant once,
before minting any `RunContext`. `Instant::from_std` transfers the original
frozen source deadline. Neither queue delay nor an entered callback restarts
that deadline, and the entered tail is not checked against it again.
Legacy scheduler entry points and kernel behavior remain unchanged.
Only this source-agent entry seals its captured agent identity into the private
`RunContext`. Generic agent/observe contexts carry no such provenance, and source
write admission requires the seal to match the callback's immutable session.

Both source-agent and source-observe entries also consume the private source
lifetime gate at `Event::Run`. Its production implementation uses only atomic
closing/admission state and a single-use callback stamp. Ordinary service close
fences callbacks that have not entered, while an entered lease drains normally;
it ignores only service closing, never actual credential/provider revocation.
Fresh caller/Scope/process checks run outside the Control mutex at native Page
admission. The entry gate performs no database access or other locking there.

The existing source distinction between foreign read observation and an agent's
own queue is retained for `waitFor`. Physical tab/current transport and pending
dialogs are checked at actual callback entry. Generation changes invalidate
references immediately through the existing watcher and again after the old
callback drains. Resource retirement stops only the worker/gateway; it does not
close the visible tab or its actual Control. Normal toolkit close must first
drain the source callback ledger before closing its executor.

## Implemented adapter and remaining router

The initial adapter routes the worker's actual `click`, `type`, `hover`, `select`,
`drag`, `press`, `scroll`, `evaluate` and `waitFor` implementations. Missing tabs
return the original tab-not-found error. Source void results are mapped by their
fixed original operation: click intentionally discards its point result;
type/hover/drag/press/scroll/waitFor return actual JavaScript `undefined` through
the private callback output. Select and evaluate retain their returned values.
Native errors are never converted into successful void results.

Status, open, navigation, resize, appearance, dialog, upload, recording and
snapshot currently return explicit operation refusals. In particular, status
and open cannot create a Page worker as a fallback. Open requires the remaining
explicit manager branch before Page capture. Snapshot's existing empty
console/network/action arrays are not admitted by this factory.

The complete original router, action recording/timeline, desktop pointer and
chooser behavior, host reconnect/live-tab reports and all twelve consumers are
still pending. The pinned complete Page evaluate implementation already maps
CDP responses without a value or description to null. Physical comparisons for
`undefined`, `void 0` and `null` return null from both the original implementation
and the adapter, so these cases demonstrate no evaluate output gap. Fixed source
void mapping remains operation-specific. Existing worker/CDP frame and
concurrency limits remain foundation limits, not native limit parity.

## Defaults and qualified evidence

The Page attachment now applies both original 15,000 ms defaults from
`ServerBrowser.ensureTab`: action timeout and navigation timeout. The new private
`page-defaults` fixture uses actual Chromium and pinned Playwright 1.60.0. With
no explicit per-call timeout, an absent locator and stalled loopback navigation
both report `Timeout 15000ms exceeded`: 15,000.11 ms and 15,000.68 ms respectively
in `page-defaults-physical-3/page-te8ybljt/proof.json`. The cases run sequentially
against actual Chromium, with no mocked Page or settings readback. All three
whole-original evaluate/adapter comparisons above also pass. The entry's nullish
projection is recorded separately; this is not a complete worker invocation.

The first two retained attempts failed and do not prove defaults. Attempt one
did not retain the underlying error; attempt two records that the Page was
already closed after the fixture's comparison-session detach. Attempt three
retains that extra owned CDP session until final cleanup and passes without
production changes. Guard receipts report no refused recursive operation and
zero executed recursive removals; Playwright artifacts are retained by rename.
These receipts accept only the physical JavaScript defaults and measured
evaluate cases, not the Rust factory, source caller or complete MCP route.

Four new private Rust cases cover expired source queues, ownership/generation
guard precedence, and an entered tail surviving observer cancellation, handoff
and its admission deadline, plus refusal to obtain source-agent provenance from
generic contexts. All four pass in the private rootless test container, including
the five-second admission-deadline tail.
Three additional service cases use the production atomic gate with actual
Control entries, reject queued/observer entries after closing, and check separate
source-end/observer-return/execution/finally accounting against actual State and
credential records. All three pass; they do not substitute for a physical CDP
receipt.

The same focused batch passes three request-class/timeout/raw-input codec cases,
two cancellation/FIFO cases, and an end-to-end owner case using actual State,
credential records, Service/Ledger, Rust Worker, admitted Node 26.13.0 and the
whole original toolkit. That case explicitly uses a synthetic callback executor:
observer cancellation ends the source before the held callback, prevents a new
metadata producer, and retains the actual late `markUsed` receipt before clean
worker closure. No Page, CDP or provider process is launched by it.

Build receipt: `page-toolkit-owner-tests-1/build-result.json`, strict library:
`page-toolkit-owner-library-2/result.json`, focused runtime:
`page-toolkit-owner-tests-1/runtime-result.json` (13 exact cases, no ignored
cases). A separate changed caller test passes 26 projection fixtures and its
then-current identity expectations
(`page-toolkit-caller-transition-1/runtime-result.json`). Primary-source review
subsequently found those identity expectations incorrect: the MCP credential's
own UUID is distinct from runtime provider-session rows, and loading the caller
requires neither an active provider thread nor a published runtime session.
The 13- and 26-case receipts remain evidence of the prior candidate's mechanics,
not original caller-identity parity; corrected caller acceptance is separate.
The source fences include the preserved unrelated `events.rs` work in this
worktree; it is excluded from this checkpoint and these receipts do not certify
a clean published tree or all default-feature targets.

The corrected credential source then passes both strict library modes and the
minimal libtest build (`page-toolkit-credential-page-build-2`). Its three exact
private runtime cases pass with source unchanged: corrected actual credential
projection/replacement, actual Node observer cancellation, and a first physical
factory evaluation slice. That slice issues a real O3 Context, traverses the
Service and whole original toolkit into the actual factory/NativePage/Gateway
and Chromium Page, then accounts for the original optional status refusal and
two actual delegated `markUsed` receipts. It verifies a clean callback ledger,
canonical physical owner and that toolkit resource closure preserves the
borrowed UI tab/legacy CDP session; no profile-removal call is executed.
No test authority constructor or synthetic CDP reply supplies the source action.
The provider records, launcher proxy and nine remaining router branches retain
their stated fixture/implementation qualifications. This is not public MCP or
complete twelve-operation acceptance.

Required physical acceptance remains:

- Two actual caller contexts on one physical Page reuse one worker and native
  keyboard state; a replacement registry/target cannot reuse that identity.
- Real source service issue/reserve/run/finally messages, original codecs and
  raw input reach the actual factory. No test-only authority constructor counts.
- A queued expired callback mints no permit, ownership/handoff/close refuse it,
  and an entered callback survives its observer and admission deadline.
- Foreign reads observe while an agent tail is blocked; an agent's own read
  preserves its original queue and pending-dialog errors.
- Closing toolkit resources preserves the physical UI tab, Control and borrowed
  legacy session; actual tab close retires the owned worker and gateway.
- Startup/cancellation failures retain independent cleanup receipts; Unknown
  refuses replacement and cannot become ordinary successful cleanup.

Run only a coordinated focused build/runtime batch in the existing target and a
private rootless container. No live profile, host display, credential, microphone
or paid provider call is part of this source checkpoint.

The clean integrated candidate subsequently passes both strict all-target
modes and the three exact credential/cancellation/physical-factory cases in
private rootless containers. See `page-toolkit-integrated-clean-2` and
`page-toolkit-all-targets-clean-1`. The two external-asset process fixtures are
explicit opt-in tests, each selected by exact name plus `--ignored`; ordinary
repository CI does not include their external pinned source/runtime mounts.
No fixture silently succeeds when those dependencies are unavailable.
