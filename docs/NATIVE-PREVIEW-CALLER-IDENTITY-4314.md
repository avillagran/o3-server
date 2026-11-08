# Native preview caller credential identity

The source preview owner key is an MCP credential identity, not a runtime
`providerSessions` record or `activeProviderThreadId`. The original registry
issues the credential before tool invocation. Capturing the existing credential
at the first actual tool invocation therefore requires no provider-publication
wait and no new provider lifecycle hook.

## Pinned original evidence

All references below are from the clean nightly 4314 source revision
`83a3674ffd84ffb33de768210c806cb1ac53fd80`.

| Source | Relevant behavior | SHA-256 |
| --- | --- | --- |
| `apps/server/src/mcp/McpSessionRegistry.ts:97–137` | `issue` generates a UUID and places it in the credential config, `scope.thread.providerSessionId`, and `requestNamespace`; no runtime projection lookup occurs. | `324094b7befdfc2cf488d95a7393948ccec55ae63fafe8c281ccd27ee89c8345` |
| `apps/server/src/orchestration-v2/ProviderSessionManager.ts:454–502` | The manager explicitly calls this identity `mcpCredentialId`, validates/reuses the credential, and replaces it when thread/provider/capability conditions change. | `0ba8717bf18a2bc0203b9bfc810d8d2fd237d0e0407d95195e0eeb0659513751` |
| `apps/server/src/mcp/threadAccess.ts:87–112,141–150` | Caller loading reads the actual thread shell without requiring an active runtime provider-thread/session. The separate action guard checks live ownership; readonly behavior is separate. | `a8e9d2ca932736b4826ae3b9166937dc7e57f2cab50c3c807b5db6da415a577a` |
| `apps/server/src/mcp/PreviewAutomationBroker.ts:164–167` | The host assignment key joins environment ID, MCP credential session ID, and application thread ID with NUL separators. | `25207d4e247dcae0b84d352890956ba9c300a0f728d098ca921805306643cf86` |

O3's corresponding immutable credential identity is the actual native MCP
`Context` ID, also used by its `Authority` request namespace. The source caller
retains this identity and the actual authority; input JSON cannot replace them.
Runtime provider-thread/session transitions, missing runtime bindings, and
`prepared`/`not_loaded` rows do not change this credential. Actual credential
revocation, process loss, idle expiry, and a missing/deleted calling thread still
fence source reads. Provider and generation changes fence actual write callbacks
without replacing the issued scope. The generic MCP read and mutation gates are
unchanged. No runtime record is manufactured to make source admission succeed.

Obtaining a Context before provider binding is not itself a source invocation.
O3's Codex binding event is queued before `turn/start`, but the bounded provider
event queue is not a database-commit acknowledgement. This ordering does not
justify adding a publication requirement that the original credential registry
does not have.

## Verification and limitations

The State/Context caller case checks that the scope and namespace use the real
credential, runtime bindings cannot retag it, and replacing/revoking the actual
credential fences the old caller. The actual Rust service/Node-entry cancellation
case no longer inserts a runtime provider-session record to obtain admission.

`tests/preview/mcp-session-identity-oracle.mjs` executes the complete original
registry with controlled environment/address/clock services and real Effect
crypto services. It composes the original issued scope with the whole original
preview toolkit to check readonly/action distinctions, foreign-provider reads,
withheld capabilities, replacement, and liveness expiration. Browser callbacks
remain explicitly synthetic. The oracle records no bearer tokens. It does not
prove propagation of a real UI settings change into provider-manager credential
replacement, a physical CDP action, or public MCP selection.

The whole-original Registry oracle passes in private rootless Node26.13.0:
`page-mcp-credential-original-2/page-11_o9n3m/result.json`. It covers fourteen
original projection states/background combinations, foreign-provider readonly
versus action guards, withheld capabilities, credential replacement and expiry.
The first receipt remains failed: its assertions completed but its fixture called
a nonexistent finalizer; only that fixture call changed to `disposeBroker` before
the new successful run.

Both strict Rust library modes and the minimal libtest build pass in
`page-toolkit-credential-page-build-2/build-result.json`. Three exact changed/new
runtime cases pass in its `runtime-result.json`: the actual credential/projection
case across 26 fixtures, actual Rust/Node observer cancellation with a synthetic
executor, and actual Context/Service/original toolkit/factory/NativePage/Gateway
through real Chromium evaluation. The latter deliberately retains the current
optional metadata-status refusal and verifies both original finally/markUsed
receipts plus borrowed UI-tab survival. It does not accept full status, all twelve
operations, public MCP selection or UI-settings propagation. Source fences retain
the unrelated `events.rs` worktree patch, excluded from the source checkpoint;
these are not clean published-tree runtime receipts.

Earlier 13-case service and 26-case projection receipts retain their original
hashes and implementation scope. Their projection/codec and cancellation findings
remain qualified. Their caller-policy assertions implemented the incorrect
runtime-publication requirement and do not prove credential-identity parity.
The earlier published caller implementation needs this correction before any
parity claim or activation.

The two earlier owner-process integration cases are explicit opt-in libtests because
ordinary repository CI does not contain the external pinned nightly, Effect,
Playwright and browser fixtures. Run each exact filter with `--ignored` only
inside the coordinated private container. They have real passing receipts above;
ordinary unit-test execution must not silently pretend those assets exist or
return success without exercising the case. The credential/projection case
remains an ordinary unit test. Public MCP selection stays closed.

Clean integration follow-up: `page-toolkit-integrated-clean-2` compiles both
strict library modes and runs all three exact cases on candidate `0c71342`,
with the two process fixtures explicitly selected using `--ignored`.
`page-toolkit-all-targets-clean-1` also passes strict all-target checks in both
modes and the full static filesystem-safety preflight. The unrelated worktree
`events.rs` patch is absent from this integrated source and remains preserved
in its original worktree. These replace the earlier worktree-contaminated
integration qualification, while preserving those earlier receipts unchanged.

## Source-specific readonly compatibility

The increment starts from published server revision `1947191` and integrates the
published profile-catalog revision `b77885f` before validation. The eight caller
files retain their exact bytes through that fast-forward.

`Scope.page_caller` validates the actual credential and reads the current
calling thread without requiring that thread's current provider or generation to
match the issued credential. `Scope.caller`, including generic readonly calls,
and the generic mutation authority retain their existing strict checks.

`Caller.capture`, `scope`, and `thread_projection` use this source-specific read
path. The descriptor retains the credential's issued thread, provider instance,
session ID, namespace, issue time, and browser capability option. It never copies
a successor provider's identity from the current thread. Capability disclosure
does not change when an issued browser backend becomes unavailable.

The toolkit's initial invocation and caller-scope service validate the issued
read without preempting the original handler's capability or live-caller checks.
The whole original `loadCaller` can therefore see the current foreign shell and
`assertLiveCaller` can return `parent_not_active` before creating a write
callback. A withheld preview capability likewise reaches the original handler
and dispatches no callback.

`SourceCallback.validate` requires the issued preview capability and fresh
backend availability. Its fixed decoded `NativeOperation` additionally requires
the unchanged generic provider/generation check for write operations. Native
Page admission and every source frame retain the actual tab/owner fences and
call this validation again. The separately decoded, fixed metadata-status
producer remains readonly; no JSON input flag selects write admission.

The focused fixtures for this increment are:

- `actual_credential_reads_complete_projection_and_retains_issued_binding`:
  all 26 original projections retain their issued scope through runtime-binding,
  current-provider, and generation changes, while generic readonly checks reject
  the foreign binding.
- `page_issued_read_identity_preserves_strict_writes_and_credential_lifetime`:
  real Context credentials cover readonly/write classification, immutable issued
  capabilities versus current availability, process loss, idle expiry,
  revocation/replacement, and deleted/missing threads. Generic read and mutation
  checks remain strict.
- `actual_context_foreign_reads_reach_original_toolkit_and_late_writes_remain_fenced`:
  the actual Rust service and Node entry run the whole pinned original toolkit,
  with an explicitly synthetic callback executor. Foreign readonly access
  succeeds, the original write prelude rejects before callbacks, and a generation
  change after callback admission prevents the held write. Withheld preview
  capability returns the original formatted error without dispatching callbacks.

The current focused evidence is:

| Receipt | Accepted scope |
| --- | --- |
| `native-readonly-caller-build-1/build-result.json` | Minimal and default strict all-target checks pass, followed by the minimal libtest build. Production source remains unchanged through the later fixture-only correction. |
| `native-readonly-caller-runtime-1/runtime-result.json` | The first two exact cases pass: all 26 projections and actual credential lifetime/capability/generic-policy checks. The third case fails its fixture's error-format assertion; the fourth case does not run. This is a partial receipt, not a successful four-case run. |
| `native-readonly-caller-build-2/build-result.json` | Minimal libtest build passes after correcting only the new process fixture's two expected error messages. Current artifact SHA-256 is `8272803e87322135ae8971e2e4f1219f29525fa414e2a6b29df1e057fda3b802`. |
| `native-readonly-caller-runtime-2/runtime-result.json` | The changed whole-toolkit case passes, followed by the previously unrun actual Context/toolkit/factory/NativePage/Gateway/Chromium evaluation case. Both process fixtures explicitly select `--ignored`; the unchanged first two cases are not repeated. |

The failed assertion expected the internal `parent_not_active` code in the MCP
wire result. The admitted original Effect 4.1.2 `declaredFailureResult` formats
an `Error` with a nonempty message as `isError: true` and text content. The
corrected fixture compares the original ownership and missing-preview messages
exactly and retains the actual JSON on a mismatch. No production error formatter
or authority check changed to satisfy the fixture. The failed receipt and its
complete log remain intact.

The generic `Scope.caller` body remains byte-identical to the public base, with
SHA-256 `6b5d75218924817f5d9a2e899f9576208f6c6374c72eecb20ef50ef6f76673c6`.
Builds use the existing coordinated target, two jobs, no incremental output, and
a 1.5 GiB free-space floor before each Cargo command. Every runtime runs inside
private rootless Podman with source, binary, Node, and dependency hashes checked
before and after. No binary/runtime copies, host tests, live profiles, public MCP
selection, or full-suite acceptance are implied.

The Chromium case validates the changed caller path through the existing real
evaluation route and retains its optional metadata-status refusal. The foreign
readonly toolkit case uses synthetic callbacks. Neither case proves complete
physical foreign-provider status, all status producers, or the remaining native
router. Actual provider-manager capability replacement and inherited/related
thread projections remain separate work.

## Newer reference equivalence

`nightly-4340-reference-20261108/equivalence.json` compares the original pinned
revision `83a3674ffd84ffb33de768210c806cb1ac53fd80` with nightly 4340 revision
`a9e709f15f825cacac1b6d665fa1920c328d68b7`. Its SHA-256 is
`833a85636b405d6eb996d1fbf95f277cfdbafec081bf5535361a674df8a208ba`.
The exact 70 listed footprints are byte-identical, including the entire MCP
directory, full ServerBrowser/Page sources, preview contract, profile reporter,
configuration projection, and presentation sources. The original checkout and
all pinned 4314 receipts remain unchanged.

This source comparison permits retaining evidence for those exact footprints.
It does not accept the whole 4340 application, changed contracts/settings,
dependencies, schemas, auth/config/provider behavior, or other changed UI code.
