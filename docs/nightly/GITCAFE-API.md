# GitCafe API request policy

The request policy is pinned to nightly 4288, revision
`32a0531748c44b25072d7495ce0b9bd7e9450a87`. The principal authentication consumer
now uses this policy rather than a separate HTTP-status shortcut. Source-control
write actions and global readiness remain unchanged.

`tools/generate-gitcafe-api-fixtures.mjs` executes the complete native
`GitCafeApi.make` and `request` methods with Effect 4.1.2 and synthetic credential
and HTTP services. Its 478 observations record the 100-file source closure,
credential reads, HTTP methods/URLs/bodies/headers, errors and invalidations.
Coverage includes the five methods, redirect statuses and targets, normalized
host allowlist, three-hop limit, credential failures, problem-field types, body
absence versus JSON null, BOM decoding, and a real thirty-second stalled-send
failure. The native error envelope is compared independently of its opaque cause.

Only `git.cafe` and `staging.git.cafe` receive credentials. Redirects remain on the
same HTTPS origin under `/api/`; GET accepts the native redirect statuses while
other methods follow only 307/308. A request reads its selected credential once;
redirects retain that credential. A final 401 invalidates its normalized host
cache. A thirty-second deadline covers each complete send, including waiting for
an O3 HTTP slot, and cancellation releases that slot.

Problem details follow the native schema and whitespace rules. A present null or
non-string known problem field invalidates the problem object. An empty detail
does not select the title instead. Truncation takes 300 UTF-16 code units and
adds an ellipsis when needed; the error stores those code units exactly, including
a surrogate split by truncation. Debug output omits response text, tokens and
credential causes. Authorization headers are marked sensitive before they enter
the physical HTTP client.

Private real-RPC proofs use a synthetic CLI and loopback HTTP. They exercise an
allowed renumbering redirect, a refused redirect to an independently listening
local trap, 401 invalidation followed by a changed CLI token, BOM decoding,
credential reuse, reference refusal, and response/account values that must not
expose a token. The trap receives no connection. No actual account, vault, SSH
session, public endpoint or host authentication store is used.

The O3 transport retains its four concurrent slots and 8 MiB response cap. This
cap is an explicit difference for oversized native responses. UTF-8 error
rendering replaces isolated UTF-16 surrogates; native code-unit observations stay
exact internally. JavaScript JSON parsing versus serde (including isolated input
surrogates and numeric limits), JSON number serialization, URL-library edge cases
outside the recorded corpus, and TLS implementations remain explicit boundaries.
Bitbucket's generic rate-limit/redirect machinery and operation-level GitCafe
repository/change-request projections are not activated by this request-policy
consumer.
