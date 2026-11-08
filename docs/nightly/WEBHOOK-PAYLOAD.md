# Native webhook payload semantics

Reference: T3 `v0.1.8-nightly.20261106.4262`, commit
`c55db3e9c72fdf584f4b54192fb39beb88667b59`. The covered scheduled-task
sources are unchanged in nightly 4275. Source hashes and evidence scope are in
`webhook-payload-intake.json`.

## Reference execution

`tools/generate-webhook-payload-fixtures.mjs <T3_CHECKOUT> <OUTPUT>` reads the
pinned files with `git show`. It executes the complete `webhookTemplate.ts` and
`webhookFilter.ts` modules after removing TypeScript types, changing only the
relative import location. The dedupe `key` function is extracted without changing
its body and uses the original renderer and Node crypto implementation.

Generation runs inside the rootless test container with source, reference Git
objects and Node mounted read-only. Only the unique evidence directory is
writable; no credentials or external network are available. The committed fixture
contains synthetic requests and signing material only.

The Rust regression compares 1,972 render cases, 1,479 filter cases and 1,015
dedupe keys. An additional reference case resolves a field through 4,096 nested
objects. Coverage includes property order, duplicate keys, integer-like property
names, own-property/prototype boundaries, malformed JSON, form bodies, URL query
duplicates/decoding, nested wildcards, missing/null/empty values, IEEE-754 rounding,
overflow, subnormals, Unicode mentions, frame tags and credential redaction.

```sh
tools/build-test-image
tools/test-scheduled-modes --unit-filter webhook_payload::tests
tools/test-scheduled-modes --default-features --unit-filter webhook_payload::tests
```

## Implementation boundary

The parser stores body values in an iterative arena. It preserves insertion order,
uses JavaScript integer-property enumeration order and retains UTF-16 string units
for JSON quoting. It does not change the repository's global `serde_json` map
ordering. JSON numbers are IEEE-754 values and use
[ryu-js's ECMAScript formatting](https://docs.rs/ryu-js/1.0.3/ryu_js/).
Scalar text is compared at its UTF-8 byte boundary; quoted object JSON retains
escaped lone surrogates. Native provider transport acceptance remains separate.

Expressions resolve body/form data, named headers, redacted whole header/query
sets and the formatted request. Filters share that resolver, including wildcard
`notEquals` and `exists` rules. Nonempty substituted values receive an unpredictable
per-request frame and the native untrusted-data footer; template mentions remain
intact. Missing fields are collected once in encounter order.

The existing O3 signed-hook path now uses the native renderer/filter and dedupe
key calculation. Coalescing uses the native default of 120 seconds. A transactional
one-time upgrade hashes stored legacy keys without changing accepted job IDs,
prompts or deadlines; a failed upgrade rolls back its completion marker and row
changes. This does not reconstruct requests that the old queue never retained.

Native request and receipt-header JSON are copied without rewriting their bytes.
Comparison hashes normalize only their outer encoding and retain header order,
because that order affects `{{request}}`. A regression exchanges order-only edits
through both ownership directions. Other JSON columns retain semantic comparison
so cosmetic schedule formatting does not create a task conflict.
Hash format 2 also upgrades the previous sorted-JSON baseline transactionally.
Unchanged native request bytes restore ordering lost by an older copy while
unexported local task edits are retained. The format marker commits with the new
hashes; unsupported future formats are refused.

## Still open

This establishes payload helpers and their integration with the existing queue.
The legacy validation restrictions, ingress response/delivery IDs, native receipt
and replay policy, native pending worker, dispatch/completion recovery, relay
verification and accepted-work migration still require their own implementation
and runtime acceptance. It does not certify full webhook execution handover or
global nightly parity. No additional capability is advertised and no live profile
or service is changed.
