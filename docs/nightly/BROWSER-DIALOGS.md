# Browser dialog consumers and source evidence

The reference is nightly 4288 (`32a0531748c44b25072d7495ce0b9bd7e9450a87`)
with Effect 4.1.2 and Playwright 1.60.0. The checked-in generator executes the
actual `ServerBrowser` dialog listeners, status projection, control broadcast and
answer body together with Playwright's actual `Dialog` and Chromium `_onDialog`
implementation. Browser, protocol, viewer and reference-invalidation services are
inert synthetic dependencies. Source hashes and the canonical Effect module URL
are retained in the fixture.

The fixture contains 125 lifecycle traces and 13 actual input-schema outcomes.
They cover replacement while answering, external desktop closure, failed answers,
repeated answers, frame admission, viewer-specific control projection and the
4,000 UTF-16-unit text boundary. The small tool fixture is extracted from the
actual native tool and Effect MCP metadata conversion, including annotations.
The implemented differential subset and runtime acceptance are described below.

The advertised native MCP JSON schema allows nullable optional `tabId` and
`promptText`, while the actual tool parameter decoder rejects null. Runtime
validation must follow the decoder rather than infer semantics from the advertised
schema. A supplied tab ID is trimmed, nonblank and limited to 128 UTF-16 units;
`accept` is required and boolean. Unsupported control/ownership fields must not
be fabricated merely to fill optional status properties.

Two source traces slice a surrogate pair at the 4,000-unit boundary and produce a
lone surrogate. Rust JSON strings cannot preserve that value. Internal unit-level
comparisons can retain the native units, but do not establish complete JavaScript
string wire parity. This boundary must remain an explicit limitation unless a
lossless JSON string codec is implemented.

Transport multiplexing already has independent inherited-pipe and desktop-WebSocket
proofs in which a dialog answer unblocks a held evaluation. That proof does not
establish native pending-dialog state, status/tool admission, viewer broadcasts,
popup ownership or safe download materialization. The state/status/answer consumers below add separate acceptance; popup and
download lifecycle work remains pending.

## Implemented consumer scope

The server now stores pending dialog state per live opaque CDP session. Opening
requires the observed page frame; detached or foreign sessions cannot mutate it.
Answer admission marks the captured dialog handled before sending the protocol
command. A failed answer remains handled, while successful completion clears only
the captured identity and preserves a replacement dialog. External desktop close
clears pending state without generating a control broadcast. Local and desktop
actors share this state machine.

`preview_dialog` uses the source-generated native metadata and validates the actual
parameter decoder outcomes. Status and answers bypass the agent's ordinary
operation gate; close also bypasses it. Agent admission remains limited to its
owned tabs. The source's broader unclaimed-tab admission, full SessionControl
ownership/generation transitions, reference invalidation, file-chooser interception
and beforeunload navigation-await cancellation are not implemented by this change.
Status derives URL, title and loading from the existing observed navigation
snapshot, with title null during a pending dialog.

The viewer reads dialog revisions independently of an outstanding screenshot and
can answer through its existing interactive authorization. Control messages retain
the existing viewer claim/generation semantics and legacy `type` discriminator,
with the native `_tag` also present. This is not complete native SessionControl
wire parity. Dialog status and answer commands do not enable additional platform
capabilities or imply popup/download lifecycle support.

Rust differential coverage compares native status UTF-16 units, answer commands,
outcomes and broadcast revision counts across all 125 traces, plus the 13 decoded
input outcomes/values. Other observations in the reference fixture, including
navigation-abort callbacks and viewer ownership projection, remain reference data
rather than claimed implemented consumers. Runtime acceptance uses synthetic
owned processes, protocol peers and framed sockets inside private containers.

## Regeneration

Use `tools/generate-preview-dialog-fixtures` with `--checkout` for the pinned
checkout, `--staged` for the verified contracts staging tree, `--deps` for its
Effect 4.1.2 dependency directory, `--node` for the Node 26 installation root,
`--cr-page` and `--dialog` for the pinned Playwright source files, and
`--evidence-root` for a private retained output directory. The wrapper requires
rootless Podman, mounts source/dependencies read-only, disables network access,
and writes only to a new retained evidence directory. It does not install tools
or run the native reference bodies on the host. The generator verifies pinned
source and staging hashes and checks canonical Effect identity before evaluation.

The checked-in wrapper regenerated all 125/13 rows byte-for-byte from the native
source. Physical acceptance additionally covers a desktop opaque-session answer,
an inherited-pipe dialog answer through a framed interactive viewer while both
an evaluation and screenshot response are held, and actual provider-authenticated
MCP metadata/status/answer admission. Full-size transport and full control-owner
wire parity remain outside these bounded tests.
