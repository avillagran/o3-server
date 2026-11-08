# Speech store/service/HTTP error boundary (4314)

The private `SpeechModelDownloadError` class in the pinned
`SpeechModelStore.ts` is not a member of `SpeechService.ts`'s `SpeechError`
union. The service therefore wraps it in `SpeechOperationError` with operation
`model download`. Preserving a dedicated Store download variant at the service
boundary was an incorrect earlier inference.

The differential fixture executes the complete unchanged native ModelStore
against an explicit virtual filesystem and HTTP status 500. The test harness
retains its actual private class instance as a non-enumerable observation; it does
not recreate that error class or patch the native module. The same instance passes
through the complete native SpeechService, and its resulting operation error goes
through the complete native voice HTTP router and error mapper.

Authentication principals and unused settings/cloud/provider dependencies are
explicit fixture bindings. No cryptographic authentication, real filesystem
removal, network listener, model inference, or microphone is exercised by that
native HTTP fixture. The original HTTP handler, middleware, payload decoding,
scope checks, service error classification, and response serialization run.

Observed results:

- An `orchestration:operate` principal cannot download a model: HTTP 403,
  `EnvironmentScopeRequiredError`, legacy `requiredScope` value
  `orchestration:operate`, and canonical `requiredPermission` value
  `settings:write`.
- A `settings:write` principal reaches the failed download: HTTP 500,
  `EnvironmentInternalError`, `code` and `reason` equal to `internal_error`.
  The private Store error and its model ID do not appear in the response.

Only the request trace ID is normalized in the golden after checking that the
native handler supplied a nonempty string. The existing fifty whole-ModelStore
observations remain byte-identical after exporting the reusable test harness.

`tests/speech_service_error.rs` checks the O3 service boundary against this native
observation using an explicitly virtual backend/HTTP status 500. It does not
claim that O3 voice HTTP routes are active: those consumers and their actual
session authorization must be accepted separately before capability exposure.
