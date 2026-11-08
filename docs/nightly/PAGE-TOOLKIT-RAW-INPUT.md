# Exact private worker inputs and terminal receipts

The inactive worker supervisor now serializes JavaScript `RawValue` directly
inside a typed invocation frame. Its existing Value-based owner APIs preserve
their behavior, and scoped raw APIs still require an actual `NativeInvocation`.
Model input fields cannot replace the outer caller, owner receipt or tool class.
A frame over the existing limit is refused without retiring the retained worker
or leaving a pending wire request.

The toolkit resource role has no Page target or endpoint. Typed continuation
frames are admitted synchronously in wire receive order and then run separately
from observer lifetimes. During close, already admitted finalizer/end messages
can still drain. The child closing is insufficient evidence that a continuation
finished: an unfinished continuation prevents a clean owner-close receipt.

Strict minimal library Clippy passes at
`/var/tmp/o3-server/page-toolkit-raw-library-2/result.json`; the first attempt's
single question-mark lint is retained separately. The library test executable
builds, and two exact tests pass in private rootless Podman with source and
binary mounted read-only and a reused pinned Node binary:
`/var/tmp/o3-server/page-toolkit-raw-tests-1/runtime-result.json`.

The tests exercise real framed Node processes: unpaired UTF-16 input/output,
same-named input fields that cannot promote authority, oversized-input refusal
followed by another invocation, and a premature child-close acknowledgment with
a still unfinished admitted continuation. The explicit peer has no CDP grant.
No binary or runtime was copied for execution.

These artifacts compile the complete current private input tree, including the
preserved unrelated `events.rs` and caller-publication work. Their source hashes
are recorded; this is not a claim that those unrelated changes were accepted or
that a clean published checkout produced the same executable. The callback
ledger/request modules and real toolkit owner remain separate integration work.
No public selector, toolkit caller envelope, real tab factory or outer MCP
transport is enabled. The original preferred-open 660-second budget and other
documented native capacity gaps remain unresolved.
