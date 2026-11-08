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
revocation/replacement, owner generation/provider mismatch, process loss, and
current capability checks still fence the caller. The generic mutation gate is
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

The two owner-process integration cases are explicit opt-in libtests because
ordinary repository CI does not contain the external pinned nightly, Effect,
Playwright and browser fixtures. Run each exact filter with `--ignored` only
inside the coordinated private container. They have real passing receipts above;
ordinary unit-test execution must not silently pretend those assets exist or
return success without exercising the case. The credential/projection case
remains an ordinary unit test. Public MCP selection stays closed.
