# MCP interoperability

`/mcp` implements the read side of the Streamable HTTP transport using MCP protocol
`2025-06-18`. This is an incremental port, not full MCP or nightly parity.

## Credentials

Create a dedicated credential against the same profile as the server:

```sh
o3-server auth mcp issue --access read-only --label 'My MCP client'
```

The command prints a secret token; pass it privately as `Authorization: Bearer …`
to the MCP client. Do not include it in logs or commit it. `--token-only` is available
for process-to-process delivery. Default lifetime is 30 days; `--ttl` uses the same
syntax as ordinary session issuance. Existing session list/revoke commands manage
these credentials too. `--help` does not create a profile or issue credentials.

MCP credentials have the `mcp-client` subject and an independent audience. Ordinary
owner/pairing bearer sessions do not authenticate `/mcp`. MCP credentials do not
authenticate the environment's ordinary HTTP, WebSocket or asset routes, including
WebSocket tickets. Expiry and revocation are checked from SQLite on every request.
Proof-bound credentials are rejected until DPoP is implemented, with no bearer
fallback. The auth row's scopes and runtime ceiling are authoritative.

`approval-required`, `auto-accept-edits` and `full-access` issuance preserve the
upstream MCP scope sets and runtime ceiling. This initial catalog contains reads
only; these access levels do not enable unpublished write operations.

## Transport

Send a JSON-RPC `initialize` request with clientInfo, capabilities and protocolVersion
using POST, Content-Type application/json and Accept application/json (or the usual
application/json, text/event-stream). The server negotiates 2025-06-18 and returns
Mcp-Session-Id. Include this header on later calls, alongside the bearer credential.
An explicit unsupported Mcp-Protocol-Version is rejected. Sessions are owned by the
credential that initialized them; their IDs grant no access by themselves.

Notifications return an empty 202. Requests support ping, tools/list and tools/call;
DELETE terminates the transport session with 204. JSON-RPC errors distinguish malformed
JSON, invalid envelopes/arguments, missing methods and tool execution failures. Tool
results contain both text JSON and matching structuredContent, with isError for failures.

There is no standalone GET SSE stream or resumable event delivery yet: GET returns
405, and an SSE-only Accept header returns 406. Requests with Origin are rejected.
There is no OAuth metadata, browser approval or dynamic registration yet. Provider
registry tokens and automatic native CLI injection still need their own lifecycle;
the older private browser broker remains separate.

Limits: 1 MiB incoming JSON, 10-second body read, eight simultaneous requests, eight
transport sessions per credential and 128 total. Inactive transport sessions expire
after 30 minutes, and all transport sessions disappear on restart. MCP bearer sessions
remain persisted and revocable across restarts. Readers run on blocking workers rather
than on async I/O workers.

## Published readers

- t3_environment_read: identity/platform and selected preferences; custom writing
  instructions are limited to 4,000 Unicode characters.
- t3_project_list / t3_project_read: live project pages and individual project settings.
- t3_thread_configuration: model selection and modes; an external client passes threadId.
- t3_thread_list: newest-updated-first project pages, title/status/settled/snoozed filters,
  lineage, modes and PR links; an external client passes projectId.
- t3_thread_search: existing bounded title/content search with optional project filters.
- t3_thread_read: durable detail, recent runs, message/activity timeline pages and text
  continuation using itemId/textOffset. Offsets count UTF-16 code units. At most 100 items,
  50 recent runs, 50,000 text units per item and roughly 2 MiB of timeline items per page;
  the first item is retained even if it alone exceeds the page budget. A shortened page
  reports hasMore and resumes after nextPosition. Item bodies are read one at a time.

References: T3 Code's MIT source at `680152e27b2a961686389ca69a1557cbb2c55f08`,
`apps/server/src/mcp/McpHttpServer.ts`, toolkit tools/handlers, McpInvocationContext,
McpToolAccess, OrchestratorMcpService, auth/EnvironmentAuth and packages/contracts.
The monitored 4119 source has the same transport protocol and credential separation;
full behavioral equivalence to that nightly has not been established.

## Evidence and remaining work

`tests/mcp.rs` exercises real loopback HTTP and the real Connection's project/thread
mutations. Tests failed before this port for the missing endpoint and MCP credentials
escaping into ordinary HTTP. Positive cases cover listing/reading real state, transcript
pagination/text continuation, Unicode, matching structured/text results, and CLI issuance.
Negative cases cover foreign transport sessions, deleted sessions, revocation, body limits,
invalid JSON/schema/envelopes, unsupported methods/tools, Origin and ordinary session
rejection. These are deterministic protocol tests, not a live nightly differential or
acceptance of the existing ChatGPT/OmaBridge integration.

Pending: provider registry tokens and native injection, OAuth/DPoP, full SSE semantics,
write access/mode enforcement, thread send/wait/interrupt/launch, task delegation,
worktree/attachment/PR/device/preview/HTML/scheduler toolkits and linked environments.
Inherited-task acknowledgement and exact activity-text rendering also need upstream
parity. No daily profile, external MCP integration or running service was changed.
