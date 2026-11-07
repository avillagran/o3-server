# Native hub account projection

The oracle for this leaf executes the complete nightly 4288 `makeCliproxyApi.readAccounts` method with the actual contract schemas, Codex and Claude usage helpers, shared window ordering and plan-label function. Controlled HTTP responses and a fixed clock supply its lower-level services. No projection algorithm is reimplemented in the oracle.

The 105 source cases cover every native plan label and account fallback, omitted versus failed credit observations, fractional durations and duration kinds, clamped utilization, model-scoped rows and ID collisions, spend-budget validation and extra-usage fallback. They also cover numeric date clipping through the JavaScript date range, extended years, ISO rollover, fractions and unqualified UTC dates. The pure Rust implementation remains independently tested against these complete-method results.

The active account consumer uses the validated API values from the existing hub client and then calls this pure projection. It omits absent public metadata, trims workspace IDs, preserves native plan precedence and sorts windows by native kind and model-ID order. Available-credit reads distinguish failure from a successful empty list. Refresh retains the existing redemption mutex; account reads have bounded fanout of four and retain input order.

The real RPC proof compares six representative observations with the independent native fixture: monthly-plan ordering, scoped rows and collisions, spend precedence, trimmed account metadata, provider failure and available-credit ordering. Only observation time is normalized. It checks the actual management request bodies, server-config stream and real quota reducer, and verifies that the management key never appears in either the snapshot or the observation.

Ten tests pass in each feature mode, including the real RPC and reset/refresh regression proofs. Strict all-target Clippy also passes in both modes.

## Remaining boundaries

This is not a complete nightly certification. The date formatter covers the JavaScript numeric range and the tested ISO/RFC forms, but other loose forms accepted by `Date.parse` remain outside its proof. Model IDs are ASCII after normalization; the recorded ordering matches the tested native default locale, rather than certifying every possible runtime collation locale. Runtime-specific TLS/error classification, body resource limits, isolated UTF-16 surrogate handling and direct provider-instance redemption remain the boundaries recorded in [CLIPROXY-NATIVE.md](CLIPROXY-NATIVE.md). The backend SDK pin is a separate compatibility checkpoint and is not changed by this leaf.
