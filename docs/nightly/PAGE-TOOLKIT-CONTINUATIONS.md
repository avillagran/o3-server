# Private toolkit entry and original callback deadlines

This checkpoint remains outside the public MCP catalog. The actual no-tab Node
entry runs the original pinned toolkit, broker and host callback, while a
synthetic continuation-service peer supplies explicit test caller records.
It does not certify the concrete Rust toolkit owner or a browser CDP grant.

The entry receives its opaque caller separately from model input. A failed
caller publication still sends exactly one source-end marker. The normal
original invocation sends its own end; the fallback never retries an ambiguous
end acknowledgment. Close waits for invocation finalizers and every reserved
callback before disposing the logical broker. An unfinished executor cannot be
certified by the child's close acknowledgment alone.

The physical entry-peer proof preserves lone UTF-16 units in both original MCP
output channels, retains the caller despite same-named model input fields, and
routes callback execution using its original stored receipt. Run/finalizer
messages no longer resend a tool class or replacement request. Receipt:
`/var/tmp/o3-server/page-toolkit-entry-peer-1/page-sdrje0xk/result.json`.

## Clock boundary

The original host passes `performance.now() + request.timeoutMs` to its control
queue. Reconstructing a new timeout after transport would extend admission.
Subtracting two floating values and multiplying by one million also fails to
prove an upper limit because both operations can round upward.

The private sender instead preserves the exact numerical comparator used by
pinned Node 26.13: `F(n) = Number(n) / 1e6`. A 64-step BigInt search finds the
first integer nanosecond whose native value meets the original deadline, and
the complete plateau consistent with a real performance sample. Two surrounding
`hrtime.bigint()` readings give signed lower/upper monotonic bounds. No epsilon,
response grace, absolute-clock Number conversion or refreshed timeout is used.
The examined definitions are the tagged original
[PerformanceNowImpl](https://github.com/nodejs/node/blob/v26.13.0/src/node_perf.cc#L345-L347)
and [HrtimeBigIntImpl](https://github.com/nodejs/node/blob/v26.13.0/src/node_process_methods.cc#L667-L674).

The separate Rust conversion captures `CLOCK_MONOTONIC`, `std::Instant`, then
`CLOCK_MONOTONIC` again. Checked signed arithmetic maps the source lower bound
against the final clock reading. This can shorten admission but cannot extend
that bound. Invalid canonical decimals, reversed bounds and unrepresentable
arithmetic fail closed. The deadline remains immutable while waiting.

The Node proof covers 1,572 threshold cases, 1,572 bounds cases, nine independent
linear references, 18 invalid inputs, 64 actual Linux measurements and eight
inclusive expiry checks. The standalone Rust fixture compiles the exact
unregistered production module and executes only in private rootless Podman:
40 Node/Rust clock samples, 18 refusals, one valid expired negative deadline and
an expiry after retaining the same deadline. The final test source only corrects
a report-field name; its earlier executed hash and correction remain separate.
Receipts and hashes are recorded in
`tests/fixtures/native-preview-toolkit-continuations-4314.json`.

## Remaining integration

`source_deadline.rs` is deliberately unregistered. The real Control callback
entry must consume its frozen deadline exactly once after ownership/generation
checks. A floating kernel deadline alone cannot prove the same bound. Transfer
to Tokio must use `Instant::from_std`, without another `now() + remaining` map;
that integration and the production Control path are not proven here.

Entered callbacks retain their actual execution receipt and unchanged native
operation timeout. A later observation timeout must not abort the entered tail.
Original `recordingStop`, dialog, close and conditional readonly-observation
branches retain their own queue/deadline distinctions. Concrete caller
publication, immutable callback ownership, actual per-tab workers, reconnect,
live-tab reporting, original bounds and outer MCP transport still gate activation.

Timeout roles also remain distinct. The original preferred-host `open` request
can use 660,000 ms and can compose a second `navigate` callback under the same
original tool. The request contract has no 120-second ceiling. Existing private
worker/entry/callback caps therefore still refuse that case; raising every Page
budget or clamping the source request would not establish parity. The actual
toolkit owner must separate its observer, factory, Control-entry and original
Page-function budgets. Original foreground broker response grace belongs to
the broker and must not be added a second time by Rust.
