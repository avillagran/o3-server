# T3 nightly 3726 wire contract for server implementers

Pinned upstream: `3f9448747bb8848050dc2cacf83affa6f94a66a1` (`v0.1.3-nightly.20261026.3726`). Research date: 2026-10-31. Paths without a root refer to `~/Work/t3-reference-3726`; `lib:` means `~/Work/lib-t3-rust`. Files missing from the sparse working tree were read from this commit with `git show`, not inferred from their imports. The active surface has **203 RPCs, 31 streams and 41 typed HTTP routes**, plus raw/media/guest/MCP routes. This is a protocol-2 spec; older orchestration APIs are not interchangeable with this version.

## 1. Bootstrap, listener, and durable state

The executable is `t3`; bare invocation, `start`, and `serve` enter the server launch paths. Runtime mode is `web` or `desktop`. CLI overrides environment configuration. Primary flags/env pairs:

| CLI | Environment | Meaning |
|---|---|---|
| `--mode` | `T3CODE_MODE` | web / desktop |
| `--port` | `T3CODE_PORT` | HTTP and WS listener; standard default 3773 |
| `--host` | `T3CODE_HOST` | bind interface; desktop defaults to 127.0.0.1 |
| `--base-dir` | `T3CODE_HOME` | explicit data root |
| `--dev-url` | `VITE_DEV_SERVER_URL` | development frontend proxy/origin |
| `--no-browser` | `T3CODE_NO_BROWSER` | suppress browser launch |
| `--bootstrap-fd` | `T3CODE_BOOTSTRAP_FD` | bootstrap envelope through inherited descriptor |
| `--auto-bootstrap-project-from-cwd` | `T3CODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD` | optionally register cwd project |
| `--log-websocket-events` / `--log-ws-events` | `T3CODE_LOG_WS_EVENTS` | event logging |
| `--tailscale-serve`, `--tailscale-serve-port` | `T3CODE_TAILSCALE_SERVE`, `T3CODE_TAILSCALE_SERVE_PORT` | HTTPS tunnel |

`cwd` is positional. Web host omission delegates binding to the HTTP runtime; service startup may deliberately default to a private-network-only 0.0.0.0 listener. Do not equate unspecified host with desktop loopback policy. Environment also includes `T3CODE_DEV_ALLOWED_ORIGINS` (comma-separated), development-only `T3CODE_DEV_AUTH_TOKEN` (at least 32 characters), log/trace and OTLP configuration. References: `apps/server/src/cli/config.ts:28`, `:109`, `:296`, `:422`; `apps/server/src/binCli.ts:66`; `apps/server/src/startupAccess.ts:1`.

Data root defaults to the resolver's home location (normally `~/.t3` (`apps/server/src/os-jank.ts:123`)); see the exact base-dir resolver citation in the source index below. `stateDir = baseDir/userdata`; implicit dev roots use `baseDir/dev`, but an explicit base dir still uses `userdata`. Layout (`apps/server/src/config.ts:159`):

```text
baseDir/
  userdata/                     # or dev/ under the condition above
    statev2.sqlite              # active DB; WAL/SHM companions while running
    state.sqlite                # older DB, retained for import
    server-runtime.json
    settings.json
    keybindings.json
    environment-id              # stable environment identity
    anonymous-id
    themes/
    attachments/
    browser-artifacts/
    secrets/<name>.bin
    logs/server.trace.ndjson
    logs/provider/events.log
    logs/terminals/
  caches/
  worktrees/
```

`server-runtime.json` is atomically replaced with a newline-terminated JSON object:

```json
{"version":1,"pid":1234,"host":"127.0.0.1","port":3773,"origin":"http://127.0.0.1:3773","startedAt":"2026-10-31T12:00:00.000Z","serverVersion":"0.1.3-nightly.20261026.3726"}
```

Optional fields: `ownerId:string`, `host:string`, `privateNetworkOnly:boolean`, `devUrl:string`, `serviceManaged:boolean`, `serverVersion:string`. `origin` is a usable URL: wildcard hosts become 127.0.0.1; IPv6 needs brackets. Record the actual listener port, not just the requested port. A stale PID/runtime file is not proof of a live server. Clear the runtime state on owned shutdown. References: `apps/server/src/serverRuntimeState.ts:12`, `:49`, `:84`, `:104`; runtime registration `apps/server/src/server.ts:773`.

SQLite setup: busy timeout 5000 ms, foreign keys ON, WAL, synchronous NORMAL, checkpoint_fullfsync ON, journal_size_limit 32 MiB; new DBs use incremental auto-vacuum. The DB migrator runs migrations 1–69. If statev2.sqlite does not exist but state.sqlite does, take a consistent SQLite backup, publish it without overwriting a concurrent destination, then migrate that copy; legacy transcript hydration remains lazy. Never copy only the live `.sqlite` file and omit its WAL. References: `apps/server/src/persistence/Sqlite.ts:12`, `:18`, `:44`; `apps/server/src/persistence/initializeV2Database.ts:17`.

**Reuse assessment:** SQLite is reusable from Rust, but file-format reuse is a much larger promise than wire compatibility. Preserve the migration ledger, shared log, receipts, auth metadata, provider bindings, JSON codecs, pending effects and projection revisions, plus `secrets/`, attachment paths and Git checkpoint refs. Concurrently replacing the writer with an implementation of only a few projection tables is unsafe. Prefer importing a consistent copy until all current migrations and reducers are implemented. This is an implementation recommendation, not an upstream incompatibility restriction. Appendix D includes migration DDL and the migration index; data-repair/backfill operations are separately identified there.

The authoritative application log is `orchestration_events`: auto-increment global `sequence`, unique `event_id`, `aggregate_kind`, `stream_id`, per-stream `stream_version`, `event_type`, timestamp, command/causation/correlation IDs, actor, payload JSON and metadata JSON. Migration 55 adds `application_event_version` and moves agent events into this shared log. `orchestration_v2_events` is a migration source, **not** the current independent sequence authority. Projects and agent/thread events share the sequence. The log's `(aggregate_kind, stream_id, stream_version)` is unique. Receipts deduplicate command IDs. Current v2 projections cover threads, runs, attempts, nodes, provider sessions/threads/turns, runtime requests, messages, plans, turn items, checkpoint scopes/checkpoints, subagents, queue/effects/workflows and scheduled tasks. `projection_projects` remains relevant; migration 63 drops selected obsolete v1 projections, not all legacy data. References: `persistence/Migrations/001_OrchestrationEvents.ts:8`, `persistence/Migrations/OrchestrationV2/ApplicationEventSource.ts:33` (both under `apps/server/src/`), `apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:8`.

## 2. HTTP and authentication

JSON endpoints use UTF-8 JSON unless stated otherwise. DateTimeUtc encodes as an ISO UTC string, not an Effect object. A missing optional property differs from explicit null. Success types in the route matrix and RPC matrix resolve in Appendix C. URL path parameters are percent-encoded. Do not expose provider secrets in responses or diagnostic causes.

### 2.1 Discovery and credential flow

`GET /.well-known/t3/environment` is unauthenticated. Response: `{environmentId,label,platform:{os,arch,machine?},serverVersion,orchestrationProtocolVersion?:2,capabilities:{repositoryIdentity:boolean,...optional capability flags}}`. OS: darwin/linux/windows/unknown; arch: arm64/x64/other. Advertise only supported features: clients gate uploads, paged shells, command output, self-update and preview on capabilities. `server.getConfig` provides additional configuration/auth/provider capability information; it is not the discovery descriptor. Sources: `packages/contracts/src/environment.ts:12`, `:93`, `:166`; `apps/server/src/http.ts:312`.

`GET /api/auth/session` can be called without credentials and returns `{authenticated:false,auth:{policy,bootstrapMethods,sessionMethods,sessionCookieName,serverUpdateScope?}}`. Authenticated responses also return `scopes`, `permissions`, `sessionMethod`, `expiresAt`. Policies are desktop-managed-local, loopback-browser, remote-reachable, unsafe-no-auth. Browser bootstrap is `POST /api/auth/browser-session` JSON `{credential:string}`; response `{authenticated:true,scopes,permissions?,sessionMethod,expiresAt}` plus the session cookie. Reject cross-origin cookie use even across same-site ports. Source: `packages/contracts/src/auth.ts:12`, `:213`, `:400`; `apps/server/src/auth/EnvironmentAuth.ts:654`, `:768`.

Native token exchange is **form URL encoded**, not JSON:

```http
POST /oauth/token
Content-Type: application/x-www-form-urlencoded
DPoP: <optional signed proof>

grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange&subject_token=<bootstrap-credential>&subject_token_type=urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap&requested_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token
```

Optional form fields: `scope` (space-separated requested permissions), `client_label`, `client_device_type` (desktop/mobile/tablet/bot/unknown), `client_os`. Response `{access_token,issued_token_type:"urn:ietf:params:oauth:token-type:access_token",token_type:"Bearer"|"DPoP",expires_in:number,scope:string}`. A bootstrap grant constrains the session; it is not a general API bearer. Ordinary bearer sessions send `Authorization: Bearer <access_token>`. Proof-bound sessions require `Authorization: DPoP <access_token>` and `DPoP: <proof>`; neither may be silently downgraded. Sources: `packages/contracts/src/auth.ts:189`, `:254`; `apps/server/src/auth/http.ts:349`; `apps/server/src/auth/EnvironmentAuth.ts:778`, `:927`.

DPoP is ES256/P-256 JWT: header `{typ:"dpop+jwt",alg:"ES256",jwk:{kty:"EC",crv:"P-256",x,y}}`, no private `d`. Claims `{htm,htu,jti,iat,ath?}`. htm is the HTTP method; htu normalizes the URL without query/fragment (WS URLs map to their HTTP equivalent). iat is epoch seconds with ±300 s clock window. ath is base64url SHA256 of the access token, or of the WS ticket when upgrading. JWK thumbprint is base64url SHA256 of canonical `{crv,kty,x,y}`. Bind future proofs to the established key and atomically reject replay of `(thumbprint,jti)`. Source: `packages/shared/src/dpop.ts:16`, `:38`, `:88`; `packages/shared/src/dpopCommon.ts:1`; `apps/server/src/auth/dpop.ts:61`.

`POST /api/auth/websocket-ticket/rpc` and `/media` need an authenticated HTTP session; no payload. Return `{ticket,expiresAt}`. Default ticket TTL is 5 minutes and ordinary session TTL is 30 days (`apps/server/src/auth/SessionStore.ts:432`); one-time pairing grant default TTL is 5 minutes (`apps/server/src/auth/PairingGrantStore.ts:249`). The bare `/api/auth/websocket-ticket` is a legacy audience form. `/ws` uses `?wsTicket=...&orchestrationProtocol=2`; a DPoP-bound RPC ticket additionally needs the upgrade proof in the `DPoP` header or `dpop` query parameter. RPC and media tickets have distinct audiences. RPC proof replay protection applies on upgrade; media tickets can be reused for images, EventSource and media sockets and intentionally are not proof-bound. Browser session cookies can also authenticate upgrades. Source: `packages/contracts/src/auth.ts:277`; `apps/server/src/auth/EnvironmentAuth.ts:1210`, `:1240`; `lib:tests/protocol.rs:176`.

Pairing management: `POST /api/auth/pairing-token` JSON `{label?:string,scopes?:string[]}` returns `{id,credential,label?,expiresAt}`; scope subset checks apply. List links (`access:read`), revoke links (`access:write`), list clients (`access:read`), revoke clients/others (`access:write`). Never let revocation of the current session pass through the single-client operation. Revocation invalidates active sockets. Exact route types are below. Granular `permissions` are authoritative; `scopes` is a filtered legacy representation. Empty permissions grant nothing; the server does not expand legacy parent scopes. Permission errors retain legacy `requiredScope` and add granular `requiredPermission`. Sources: `packages/contracts/src/auth.ts:98`, `:126`, `:362`; `apps/server/src/auth/http.ts:430`; `apps/server/src/ws.ts:3414`.

HTTP common errors are tagged objects: 400 `{_tag:"EnvironmentRequestInvalidError",code:"invalid_request",reason,traceId}`; 401 `{_tag:"EnvironmentAuthInvalidError",code:"auth_invalid",reason:"missing_credential"|"invalid_credential",dpopFailureReason?,traceId}`; 403 scope error `{_tag:"EnvironmentScopeRequiredError",code:"insufficient_scope",requiredScope,requiredPermission?,traceId}`; operation-forbidden 403; not-found 404 with reason `thread_not_found`; internal 500 with a stable reason and traceId. Decode/legacy cloud routes also have tagged `{message}` errors. Do not turn all failures into HTTP 200. Source: `packages/contracts/src/environmentHttp.ts:104`.

### 2.2 Complete typed HTTP route matrix

`session` below means middleware `EnvironmentAuthenticatedAuth` (cookie, bearer or DPoP as above); `public` means no such middleware, **not** that application-level proof/code/credential validation is omitted. Matrix lists actual declared routes, including OAuth, Connect and webhooks. All listed response names are wire types, not opaque implementation classes.

| Method/path | Auth | Body | Success | Query | Source line (environmentHttp.ts) |
|---|---|---|---|---|---|
| `GET /.well-known/t3/environment` | public | `—` | `ExecutionEnvironmentDescriptor` | `—` | 531 |
| `GET /api/auth/session` | public | `—` | `AuthSessionState` | `—` | 538 |
| `POST /api/auth/browser-session` | public | `AuthBrowserSessionRequest` | `AuthBrowserSessionResult` | `—` | 545 |
| `POST /oauth/token` | public | `AuthTokenExchangeRequest` | `AuthAccessTokenResult` | `—` | 552 |
| `POST /api/auth/websocket-ticket` | session | `—` | `AuthWebSocketTicketResult` | `—` | 560 |
| `POST /api/auth/websocket-ticket/:use` | session | `—` | `AuthWebSocketTicketResult` | `—` | 567 |
| `POST /api/auth/pairing-token` | session | `AuthCreatePairingCredentialInput` | `AuthPairingCredentialResult` | `—` | 575 |
| `GET /api/auth/pairing-links` | session | `—` | `Array(AuthPairingLink)` | `—` | 583 |
| `POST /api/auth/pairing-links/revoke` | session | `AuthRevokePairingLinkInput` | `AuthPairingLinkRevokeResult` | `—` | 590 |
| `GET /api/auth/clients` | session | `—` | `Array(AuthClientSession)` | `—` | 598 |
| `POST /api/auth/clients/revoke` | session | `AuthRevokeClientSessionInput` | `AuthClientSessionRevokeResult` | `—` | 605 |
| `POST /api/auth/clients/revoke-others` | session | `—` | `AuthOtherClientSessionsRevokeResult` | `—` | 613 |
| `GET /.well-known/oauth-protected-resource` | public | `—` | `AuthMcpProtectedResourceMetadata` | `—` | 622 |
| `GET /.well-known/oauth-protected-resource/mcp` | public | `—` | `AuthMcpProtectedResourceMetadata` | `—` | 627 |
| `GET /.well-known/oauth-authorization-server` | public | `—` | `AuthMcpAuthorizationServerMetadata` | `—` | 632 |
| `POST /oauth/mcp/register` | public | `AuthMcpClientRegistration` | `AuthMcpRegisteredClient` | `—` | 637 |
| `GET /oauth/mcp/authorize` | public | `—` | `HTML / redirect` | `—` | 643 |
| `POST /oauth/mcp/approval` | public | `AuthMcpAuthorizationRequest` | `[AuthMcpApprovalDetails, AuthMcpApprovalRedirect]` | `—` | 645 |
| `POST /oauth/mcp/decision` | public | `AuthMcpApprovalDecisionRequest` | `AuthMcpApprovalRedirect` | `—` | 652 |
| `POST /oauth/mcp/token` | public | `AuthMcpTokenRequest` | `AuthMcpTokenResult` | `—` | 659 |
| `GET /api/orchestration/shell` | session | `—` | `OrchestrationV2ShellSnapshot` | `EnvironmentOrchestrationShellSnapshotQuery` | 697 |
| `GET /api/orchestration/threads/:threadId` | session | `—` | `OrchestrationV2ThreadDetailSnapshot` | `—` | 705 |
| `GET /api/orchestration/threads/:threadId/transcript` | session | `—` | `OrchestrationV2ThreadTranscript` | `—` | 713 |
| `GET /api/orchestration/threads/:threadId/bounded` | session | `—` | `OrchestrationV2ThreadBoundedSnapshot` | `EnvironmentOrchestrationThreadBoundedSnapshotQuery` | 721 |
| `GET /api/orchestration/threads/:threadId/history` | session | `—` | `OrchestrationV2ThreadHistoryPage` | `EnvironmentOrchestrationThreadHistoryQuery` | 730 |
| `GET /api/projects` | session | `—` | `ProjectSnapshot` | `—` | 741 |
| `POST /api/projects/mutate` | session | `ProjectMutation` | `Project` | `—` | 748 |
| `POST /api/pull-requests/diff` | session | `PullRequestDiffInput` | `PullRequestDiffResult` | `—` | 757 |
| `POST /api/connect/link-proof` | session | `RelayLinkProofRequest` | `RelayEnvironmentLinkProof` | `—` | 773 |
| `POST /api/connect/relay-config` | session | `RelayEnvironmentConfigRequest` | `EnvironmentCloudRelayConfigResult` | `—` | 781 |
| `GET /api/connect/link-state` | session | `—` | `EnvironmentCloudLinkStateResult` | `—` | 789 |
| `POST /api/connect/reachability` | session | `—` | `EnvironmentConnectReachability` | `—` | 796 |
| `POST /api/connect/unlink` | session | `—` | `EnvironmentCloudRelayConfigResult` | `—` | 803 |
| `POST /api/connect/preferences` | session | `EnvironmentCloudPreferencesRequest` | `EnvironmentCloudLinkStateResult` | `—` | 810 |
| `POST /api/t3-connect/health` | public | `RelayCloudEnvironmentHealthRequest` | `RelayEnvironmentHealthResponse` | `—` | 818 |
| `POST /api/connect/mint-credential` | public | `RelayCloudMintCredentialRequest` | `RelayEnvironmentMintResponse` | `—` | 825 |
| `POST /api/t3-connect/mint-credential` | public | `RelayCloudMintCredentialRequest` | `RelayEnvironmentMintResponse` | `—` | 832 |
| `POST /api/hooks/:hookId/:token` | public | `—` | `{deliveryId:string}, HTTP 202; raw body` | `—` | 854 |
| `PUT /api/hooks/:hookId/:token` | public | `—` | `{deliveryId:string}, HTTP 202; raw body` | `—` | 855 |
| `PATCH /api/hooks/:hookId/:token` | public | `—` | `{deliveryId:string}, HTTP 202; raw body` | `—` | 856 |
| `GET /api/hooks/:hookId/:token` | public | `—` | `{deliveryId:string}, HTTP 202; raw body` | `—` | 857 |

### 2.3 Raw routes, uploads, assets and auxiliary transports

- `GET /ws`: Effect RPC upgrade. Require `orchestrationProtocol=2` **before** authentication; absent/wrong versions return HTTP 426 JSON `{code:"orchestration_protocol_incompatible",message,orchestrationProtocolVersion:2}`. `clientSurface`, `clientAppVersion`, device/OS/browser/deployment and connection metadata query fields are telemetry, not credentials. Source: `apps/server/src/ws.ts:630`, `:3309`.
- `GET /api/assets/*`: signed URL from `assets.createUrl`; carries resource claims and expiry. Response is file bytes or rendered MCP HTML, not JSON. Resolve the signed resource, enforce expiry/path/type restrictions; asset errors map to 400/403/404/500. A client must use `relativeUrl` verbatim; token layout is server-owned, so an alternative signer may use another opaque token if mint/validate agree. Resources: workspace-file, media-file, draft-workspace-file, attachment, tool-output-image, project-favicon, native-app-icon, github-media. `assets.createUrl` returns `{relativeUrl,expiresAt:number,sourcePath?,imageDimensions?:{width,height},fileRevision?}`. Source: `apps/server/src/http.ts:397`; `apps/server/src/assets/AssetAccess.ts:71`; `packages/contracts/src/assets.ts:22`.
- `POST /api/attachments/upload/*`: raw bytes, **not multipart**; opaque upload token is the path suffix. Mint with `attachments.createUploadUrl` using `{type?:"image",name,mimeType,sizeBytes}` or `{type:"file",name,mimeType,sizeBytes}`; get `{attachmentId,relativeUrl,expiresAt}`. URL TTL 10 minutes. Maximum image 10 MiB; arbitrary file 50 MiB. Verify the exact declared byte count and content constraints, write through pending storage, reject empty/invalid tokens and size mismatches. Successful upload returns HTTP 204. Pending uploads are deleted by `attachments.delete` or expiry sweep; command attachment persistence moves them into durable thread use. Source: `apps/server/src/http.ts:455`; `apps/server/src/assets/AttachmentUpload.ts:78`, `:159`; `apps/server/src/attachmentStore.ts:1`; `packages/contracts/src/chatAttachment.ts:15`.
- `POST /api/observability/v1/traces`: OTLP pass-through under authenticated permission checking, not a general open proxy; only the configured upstream endpoint. Source: `apps/server/src/http.ts:288`, `:331`.
- `GET /api/guest/session`, `POST /api/guest/redeem`, `POST /api/guest/messages`, `GET /api/guest/stream`, `/api/guest/render`, `/api/guest/item`, `/api/guest/command-output`, `/api/guest/attachment`: separate share-scoped guest cookie, not owner credentials. Redeem accepts GuestRedeemRequest; message accepts GuestSubmitRequest JSON or multipart with attachments; stream is SSE with 20 s heartbeat, no-store. Query `share` selects share, `item` selects item/output, attachment selects guest-authorized file. Redeem sets a Secure, HttpOnly, SameSite=Strict cookie scoped to /api/guest (24 h share TTL); redeem, session-check and message submission return HTTP 204 without a body. Stream unions are in the dictionary. Error JSON `{reason,message}`, status 403 for invalid/inactive/unavailable, 400 for empty/too_many_files/too_large/unsupported_file, 409 otherwise. Guest text max 4000 characters, name 40, at most four image attachments, total attachment budget 10 MiB. Owner approval is required before a pending guest message becomes message.dispatch. Source: `apps/server/src/guest/http.ts:49`, `:130`, `:157`, `:200`, `:222`; `packages/contracts/src/guestShare.ts:18`.
- `* /api/device-hub/*`: authenticated proxy to the device hub; upgrade allowlist: downstream `/api/devices/ws`, `/vendor/serve-sim/helper/ws`, `/vendor/serve-emu/ws`; only `/api/devices/ws` gets the read-only socket treatment; do not make this an arbitrary upstream proxy. Source: `apps/server/src/device/DeviceHubProxy.ts:162`, `:202`; `apps/server/src/device/DeviceService.ts:66`.
- `/api/preview-stream/ws` (GET upgrade), `/api/preview-stream/download` (GET), `/api/preview-stream/upload` (POST): media authentication via route interceptor, `threadId`/`tabId` selectors, download `id`, upload `chooser`; uploads are multipart. Viewing requires the appropriate read scope; interactive input additionally requires preview:operate and may be disabled with `interactive=false`. This is a separate browser media/control protocol, **not** Effect RPC. Source: `apps/server/src/preview/ServerBrowserStream.ts:28`, `:59`, `:225`, `:288`; `apps/server/src/auth/http.ts:167`.
- `/mcp`: MCP Streamable HTTP via the MCP library, including MCP sessions, JSON-RPC and its own OAuth authenticator; not the Effect RPC socket. Discovery/registration/authorization/approval/token routes are in the typed route matrix. Source: `apps/server/src/mcp/McpHttpServer.ts:1039`.
- CLI cloud login also opens a separate temporary loopback `GET /callback?code=...&state=...` listener (PKCE state-checked); this is not mounted on the main server. Source: `apps/server/src/cloud/CliTokenManager.ts:513`.
- `GET *`: packaged assets and SPA fallback / dev proxy. Static asset behavior includes content types, cache metadata and safe path resolution; unknown application paths may resolve to frontend HTML, not JSON API data. OPTIONS/CORS and HTTP compression are middleware, not additional RPC methods. Source: `apps/server/src/http.ts:249`, `:255`, `:718`; registration `apps/server/src/server.ts:704`.

Orchestration HTTP reads require `orchestration:read` and header `x-t3-orchestration-protocol: 2`. `/shell` accepts `omitIdleSubagents=1`, `omitOlderSettled=1`; `/bounded` accepts `compactTurnItems=1`, `dedupeBodies=1`, `compactIds=1`; `/history` requires opaque `cursor`, supports `throughEntryId`, `view=conversation|activity`, `compactIds=1`. No opt-in means ordinary expanded JSON. Transcript is an export read with its own size error (`thread_transcript_too_large`); bounded/history reads are the scalable interactive path. Project snapshot needs read scope; project mutations use the payload-dependent rules below. Connect routes need their relay permissions or cloud proof credentials; a missing EnvironmentAuthenticatedAuth middleware never bypasses cloud proof validation. Sources: `packages/contracts/src/environmentHttp.ts:685`; `apps/server/src/orchestration-v2/http.ts:1`; `apps/server/src/project/http.ts:1`; `apps/server/src/cloud/http.ts:1`.

## 3. Exact WebSocket Effect RPC framing

This uses Effect **4** JSON serialization (`RpcSerialization.layerJson`), not JSON-RPC 2.0, Socket.IO, newline-delimited JSON, or a custom event-name envelope. One WebSocket text message holds a JSON envelope; the serializer also accepts an array of envelopes as a batch. No `jsonrpc` property, `method`, or `params` in the Effect envelope. The socket multiplexes requests by id. Never stringify request IDs twice. Clients commonly send string IDs; the underlying envelope accepts string or number and responses preserve them. `headers` is an array of pairs, not an object. It is per-RPC metadata; HTTP authentication happens at socket upgrade.

| Direction | Wire JSON shape |
|---|---|
| client → server | `{_tag:"Request",id:string|number,tag:string,payload:JSON,headers:[ [string,string], ... ],traceId?:string,spanId?:string,sampled?:boolean}` |
| server → client | `{_tag:"Chunk",requestId:string|number,values:[successItem,...]}` (non-empty array) |
| client → server | `{_tag:"Ack",requestId:string|number}` |
| server → client | `{_tag:"Exit",requestId:string|number,exit:{_tag:"Success",value:successValue}}` |
| server → client | `{_tag:"Exit",requestId:string|number,exit:{_tag:"Failure",cause:[reason,...]}}` |
| client → server | `{_tag:"Interrupt",requestId:string|number}` |
| client → server | `{_tag:"Eof"}` (input complete, not stream ACK) |
| keepalive | `{_tag:"Ping"}` → `{_tag:"Pong"}` |
| fatal connection signal | `{_tag:"Defect",defect:JSON}` |

Failure `cause` is an **array**, not an Effect-3 Sequential/Parallel cause tree. Each reason is `{_tag:"Fail",error:<declared error>}`, `{_tag:"Die",defect:JSON}`, or `{_tag:"Interrupt",fiberId?:number|null}`. Tagged domain errors contain `_tag` and their declared fields; many also encode `message`, optional `cause` or details. A failure is terminal only for that request. T3 configures `disableFatalDefects:true` and `disableTracing:true`, so handler defects should not cancel all other requests on a socket. Unknown RPC tags produce a request failure/defect, not successful null. A void success omits the undefined `value` in serialized JSON. A stream emits zero or more Chunk envelopes followed by one Exit on termination; long-lived subscriptions normally never complete successfully during normal use. Streams return **items**, not a collected array. The client ACKs once per Chunk after processing all values. ACK flow control must not block unrelated request fibers. Output backpressure has a terminal-specific window adapter: up to 8 pending chunks or 64 KiB; the listener queue budget is 8 MiB including a 128-byte per-event overhead (`apps/server/src/terminal/OutputProtocol.ts:9`, `:75`).

Exact envelope source: `.repos/effect-smol/packages/effect/src/rpc/RpcMessage.ts:68`, `:270`, `:288`; JSON framing: `.repos/effect-smol/packages/effect/src/rpc/RpcSerialization.ts:130`; server configuration: `apps/server/src/ws.ts:3289`, `:3345`. Raw WS control Ping/Pong frames are independent from these JSON Ping/Pong messages and should also work.

Concrete fixture-derived exchange (values retained from Rust tests):

```json
{"_tag":"Request","id":"1","tag":"orchestration.subscribeThread","payload":{"threadId":"t","afterSequence":7},"headers":[]}
{"_tag":"Ping"}
{"_tag":"Pong"}
{"_tag":"Chunk","requestId":"1","values":[{"kind":"event","sequence":8,"event":{"type":"message.updated","payload":{"id":"m","text":"instant"}}}]}
{"_tag":"Ack","requestId":"1"}
{"_tag":"Interrupt","requestId":"1"}
```

The deliberately minimal event body above tests transport behavior, not the complete message.updated domain schema. Source: `lib:tests/stream.rs:6`; request envelope `lib:src/stream.rs:112`. Unary success and failure fixtures:

```json
{"_tag":"Exit","requestId":"1","exit":{"_tag":"Success","value":{"accepted":true}}}
{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"ProjectBusy","message":"Project has active runs\nat stack","secret":"PRIVATE"}}]}}
```

These test values are synthetic; accepted:true is not the orchestration dispatch result (which is `{sequence}`). Source: `lib:tests/protocol.rs:191`; `lib:tests/rpc_errors.rs:34`. They establish framing/error handling, not permission to return arbitrary fields. Rust stream liveness policy is Ping every 5 s, timeout after 20 s without any frame, explicit probe 3 s; these are client policy, not negotiated server fields (`lib:src/stream.rs:4`).

## 4. RPC catalog and authorization

Every active Rpc.make declaration is listed below, with the **encoded payload**, encoded unary result or stream-item type, declared errors, and permission. All RPCs additionally inherit `EnvironmentAuthorizationError` from group middleware, even if omitted from their local error union. No declared success means void; no declared local error means Never. Object payloads shown inline are schemas for an object, not a wrapper around that object. `Schema` names in source have been converted to the declarative grammar of Appendix C.

| RPC tag | Payload | Success / stream item | Local error | Kind | Required permission | rpc.ts line |
|---|---|---|---|---|---|---|
| `agentSessions.import` | `AgentSessionImportInput` | `AgentSessionImportResult` | `Union([ AgentSessionImportProjectChangedError, AgentSessionImportProjectNotFoundError, AgentSessionScanError, EnvironmentAuthorizationError, ])` | unary | `orchestration:operate` | 1294 |
| `agentSessions.scan` | `AgentSessionScanInput` | `AgentSessionScanResult` | `Union([AgentSessionScanError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1288 |
| `assets.createUrl` | `AssetCreateUrlInput` | `AssetCreateUrlResult` | `Union([AssetAccessError, EnvironmentAuthorizationError])` | unary | `orchestration:read + dynamic rules` | 1305 |
| `assets.persistChatAttachments` | `PersistChatAttachmentsInput` | `PersistChatAttachmentsResult` | `Union([PersistChatAttachmentsError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1311 |
| `attachments.createUploadUrl` | `AttachmentCreateUploadUrlInput` | `AttachmentCreateUploadUrlResult` | `Union([AttachmentUploadSigningKeyError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1317 |
| `attachments.delete` | `AttachmentDeleteInput` | `Void` | `EnvironmentAuthorizationError` | unary | `orchestration:operate` | 1323 |
| `cloud.getRelayClientStatus` | `Object({})` | `RelayClientStatusSchema` | `EnvironmentAuthorizationError` | unary | `relay:read` | 927 |
| `cloud.installRelayClient` | `Object({})` | `RelayClientInstallProgressEventSchema` | `Union([RelayClientInstallFailedError, EnvironmentAuthorizationError])` | stream | `relay:write` | 933 |
| `device.action` | `DeviceActionInput` | `DeviceDetail` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1621 |
| `device.close` | `DeviceCloseInput` | `Void` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1605 |
| `device.configure` | `DeviceConfigureInput` | `DeviceServiceState` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `settings:write` | 1593 |
| `device.detail` | `DeviceDetailInput` | `DeviceDetail` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1615 |
| `device.easAccount` | `DeviceEasAccountInput` | `DeviceEasAccount` | `Union([DeviceEasAccountError, EnvironmentAuthorizationError])` | unary | `settings:write` | 1581 |
| `device.list` | `DeviceListInput` | `DeviceServiceState` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:read + dynamic rules` | 1587 |
| `device.open` | `DeviceOpenInput` | `DeviceSession` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1599 |
| `device.shutdown` | `DeviceShutdownInput` | `Void` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1610 |
| `device.testHost` | `SshDeviceHostConfig` | `DeviceHostSummary` | `Union([DeviceError, EnvironmentAuthorizationError])` | unary | `settings:write` | 1575 |
| `filesystem.browse` | `FilesystemBrowseInput` | `FilesystemBrowseResult` | `Union([FilesystemBrowseError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1282 |
| `filesystem.getMetadata` | `FilesystemGetMetadataInput` | `FilesystemGetMetadataResult` | `EnvironmentAuthorizationError` | unary | `filesystem:read` | 1276 |
| `git.preparePullRequestThread` | `GitPreparePullRequestThreadInput` | `GitPreparePullRequestThreadResult` | `Union([GitManagerServiceError, EnvironmentAuthorizationError])` | unary | `source-control:write + dynamic rules` | 1405 |
| `git.resolvePullRequest` | `GitPullRequestRefInput` | `GitResolvePullRequestResult` | `Union([GitManagerServiceError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1399 |
| `git.runStackedAction` | `GitRunStackedActionInput` | `GitActionProgressEvent` | `Union([GitManagerServiceError, EnvironmentAuthorizationError])` | stream | `source-control:write + dynamic rules` | 1392 |
| `guestShare.create` | `GuestShareCreateInput` | `GuestShareCreateResult` | `Union([GuestShareCreateError, EnvironmentAuthorizationError])` | unary | `access:write` | 1862 |
| `mcpApps.callTool` | `McpAppCallToolInput` | `McpAppCallToolResult` | `Union([McpAppRequestError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1328 |
| `mcpApps.readResource` | `McpAppReadResourceInput` | `McpAppReadResourceResult` | `Union([McpAppRequestError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1345 |
| `mcpApps.toolInfo` | `McpAppToolInfoInput` | `McpAppToolInfo` | `Union([McpAppRequestError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1334 |
| `mcpApps.updateModelContext` | `McpAppUpdateModelContextInput` | `Void` | `Union([McpAppRequestError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1340 |
| `orchestration.dispatchCommand` | `OrchestrationV2Command` | `OrchestrationV2DispatchCommandResult` | `Union([OrchestrationV2DispatchCommandError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1634 |
| `orchestration.getArchivedShellSnapshot` | `Object({})` | `OrchestrationV2ArchivedShellSnapshot` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1677 |
| `orchestration.getFullThreadDiff` | `OrchestrationGetFullThreadDiffInput` | `OrchestrationGetFullThreadDiffResult` | `Union([OrchestrationGetFullThreadDiffError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1646 |
| `orchestration.getOlderSettledThreads` | `OrchestrationV2GetOlderSettledThreadsInput` | `OrchestrationV2OlderSettledThreadsPage` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1701 |
| `orchestration.getThreadFamily` | `OrchestrationV2GetThreadFamilyInput` | `OrchestrationV2ThreadFamily` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1686 |
| `orchestration.getThreadProjection` | `OrchestrationV2GetThreadProjectionInput` | `OrchestrationV2ThreadProjection` | `Union([OrchestrationV2GetThreadProjectionError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1710 |
| `orchestration.getTurnDiff` | `OrchestrationGetTurnDiffInput` | `OrchestrationGetTurnDiffResult` | `Union([OrchestrationGetTurnDiffError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1640 |
| `orchestration.getTurnItem` | `OrchestrationV2GetTurnItemInput` | `OrchestrationV2GetTurnItemResult` | `Union([OrchestrationV2GetThreadProjectionError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1734 |
| `orchestration.getWorkflowScript` | `OrchestrationV2GetWorkflowScriptInput` | `OrchestrationV2GetWorkflowScriptResult` | `Union([OrchestrationGetWorkflowScriptError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1725 |
| `orchestration.getWorktreeThreadIds` | `OrchestrationV2GetWorktreeThreadIdsInput` | `OrchestrationV2WorktreeThreadIds` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1692 |
| `orchestration.launchThread` | `OrchestrationV2ThreadLaunchInput` | `OrchestrationV2ThreadLaunchResult` | `Union([OrchestrationV2ThreadLaunchError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1740 |
| `orchestration.searchThread` | `OrchestrationV2SearchThreadInput` | `OrchestrationV2SearchThreadResult` | `Union([OrchestrationV2SearchThreadError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1655 |
| `orchestration.searchThreadStream` | `OrchestrationV2SearchThreadInput` | `OrchestrationV2SearchThreadResult` | `Union([OrchestrationV2SearchThreadError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1661 |
| `orchestration.searchThreads` | `OrchestrationV2SearchThreadsInput` | `OrchestrationV2SearchThreadsResult` | `Union([OrchestrationSearchThreadsError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1671 |
| `orchestration.stopWorkflow` | `OrchestrationV2StopWorkflowInput` | `Void` | `Union([OrchestrationV2StopWorkflowError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1719 |
| `orchestration.subscribeArchivedShell` | `Object({})` | `OrchestrationV2ArchivedShellStreamItem` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1746 |
| `orchestration.subscribeCommandOutput` | `OrchestrationV2SubscribeCommandOutputInput` | `OrchestrationV2CommandOutputFrame` | `Union([OrchestrationV2CommandOutputError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1770 |
| `orchestration.subscribeShell` | `OrchestrationV2SubscribeShellInput` | `OrchestrationV2ShellStreamItem` | `Union([OrchestrationV2GetShellSnapshotError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1756 |
| `orchestration.subscribeThread` | `OrchestrationV2SubscribeThreadInput` | `OrchestrationV2ThreadStreamItem` | `Union([OrchestrationV2GetThreadProjectionError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1763 |
| `preview.adjust` | `PreviewAdjustInput` | `PreviewSessionSnapshot` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1522 |
| `preview.clearProfile` | `PreviewClearProfileInput` | `Void` | `Union([PreviewClearProfileError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1544 |
| `preview.close` | `PreviewCloseInput` | `Void` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1533 |
| `preview.list` | `PreviewListInput` | `PreviewListResult` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 1538 |
| `preview.navigate` | `PreviewNavigateInput` | `PreviewSessionSnapshot` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1510 |
| `preview.open` | `PreviewOpenInput` | `PreviewSessionSnapshot` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1504 |
| `preview.refresh` | `PreviewRefreshInput` | `Void` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1528 |
| `preview.reportProfiles` | `PreviewReportProfilesInput` | `Void` | `EnvironmentAuthorizationError` | unary | `preview:operate` | 1549 |
| `preview.reportStatus` | `PreviewReportStatusInput` | `Void` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1554 |
| `preview.resize` | `PreviewResizeInput` | `PreviewSessionSnapshot` | `Union([PreviewError, EnvironmentAuthorizationError])` | unary | `preview:operate` | 1516 |
| `projectClone.cancel` | `ProjectCloneActionInput` | `ProjectCloneActionResult` | `EnvironmentAuthorizationError` | unary | `source-control:write` | 1192 |
| `projectClone.retry` | `ProjectCloneActionInput` | `ProjectCloneActionResult` | `Union([SourceControlRepositoryError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1198 |
| `projectClone.start` | `ProjectCloneStartInput` | `ProjectCloneStartResult` | `Union([ SourceControlRepositoryError, OrchestrationDispatchCommandError, EnvironmentAuthorizationError, ])` | unary | `source-control:write` | 1182 |
| `projects.createNew` | `ProjectCreateNewInput` | `ProjectCreateNewResult` | `Union([OrchestrationDispatchCommandError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1265 |
| `projects.ensureScratch` | `Object({})` | `ProjectEnsureScratchResult` | `Union([OrchestrationDispatchCommandError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1259 |
| `projects.listEntries` | `ProjectListEntriesInput` | `ProjectListEntriesResult` | `Union([ProjectListEntriesError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1229 |
| `projects.listIgnoredFiles` | `ProjectListIgnoredFilesInput` | `ProjectListIgnoredFilesResult` | `Union([ProjectListEntriesError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1235 |
| `projects.mutate` | `ProjectMutation` | `Project` | `Union([ProjectMutationError, EnvironmentAuthorizationError])` | unary | `orchestration:operate + dynamic rules` | 1253 |
| `projects.readFile` | `ProjectReadFileInput` | `ProjectReadFileResult` | `Union([ProjectReadFileError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1241 |
| `projects.searchContents` | `ProjectSearchContentsInput` | `ProjectSearchContentsResult` | `Union([ProjectSearchContentsError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1223 |
| `projects.searchEntries` | `ProjectSearchEntriesInput` | `ProjectSearchEntriesResult` | `Union([ProjectSearchEntriesError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1217 |
| `projects.writeFile` | `ProjectWriteFileInput` | `ProjectWriteFileResult` | `Union([ProjectWriteFileError, EnvironmentAuthorizationError])` | unary | `filesystem:write` | 1247 |
| `provider.auth.cancel` | `ProviderAuthCancelInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 689 |
| `provider.auth.complete` | `ProviderAuthCompleteInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 660 |
| `provider.auth.logout` | `ProviderSetupInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 695 |
| `provider.auth.respond` | `ProviderAuthRespondInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 654 |
| `provider.auth.start` | `ProviderAuthStartInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 648 |
| `provider.auth.subscribe` | `ProviderSetupInput` | `ProviderAuthState` | `ProviderSetupRpcError` | stream | `providers:manage` | 701 |
| `provider.chatgpt.handoff.subscribe` | `ChatGptHandoffInput` | `ChatGptHandoffState` | `ProviderSetupRpcError` | stream | `providers:manage` | 676 |
| `provider.chatgpt.import-profile` | `ChatGptImportProfileInput` | `ProviderAuthState` | `ProviderSetupRpcError` | unary | `providers:manage` | 671 |
| `provider.chatgpt.reconnect-profile` | `ChatGptReconnectProfileInput` | `Nullable(ChatGptReconnectProfile)` | `ProviderSetupRpcError` | unary | `providers:manage` | 666 |
| `provider.codex.auth-callback.subscribe` | `CodexAuthCallbackInput` | `CodexAuthCallbackState` | `ProviderSetupRpcError` | stream | `providers:manage` | 682 |
| `provider.consumeResetCredit` | `ProviderConsumeResetCreditInput` | `ProviderConsumeResetCreditResult` | `Union([ProviderSetupError, UsageLimitSourceError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 642 |
| `provider.install.cancel` | `ProviderInstallCancelInput` | `ProviderInstallState` | `ProviderSetupRpcError` | unary | `providers:manage` | 714 |
| `provider.install.remove` | `ProviderSetupInput` | `ProviderInstallState` | `ProviderSetupRpcError` | unary | `providers:manage` | 727 |
| `provider.install.start` | `ProviderSetupInput` | `ProviderInstallState` | `ProviderSetupRpcError` | unary | `providers:manage` | 708 |
| `provider.install.subscribe` | `ProviderSetupInput` | `ProviderInstallState` | `ProviderSetupRpcError` | stream | `orchestration:read` | 720 |
| `provider.uploadFeedback` | `ProviderUploadFeedbackInput` | `ProviderUploadFeedbackResult` | `Union([ProviderUploadFeedbackError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1351 |
| `pullRequests.activity` | `PullRequestRef` | `PullRequestActivity` | `PullRequestRpcError` | unary | `orchestration:read` | 1029 |
| `pullRequests.changeBreakdown` | `PullRequestRef` | `PullRequestChangeBreakdown` | `PullRequestRpcError` | unary | `orchestration:read` | 1065 |
| `pullRequests.checks` | `PullRequestRef` | `Nullable(PullRequestChecks)` | `PullRequestRpcError` | unary | `orchestration:read` | 1023 |
| `pullRequests.comment` | `PullRequestCommentInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1089 |
| `pullRequests.detail` | `PullRequestRef` | `PullRequestDetail` | `PullRequestRpcError` | unary | `orchestration:read` | 1011 |
| `pullRequests.diffFileContents` | `PullRequestDiffFileContentsInput` | `PullRequestDiffFileContentsResult` | `PullRequestRpcError` | unary | `orchestration:read` | 1053 |
| `pullRequests.filesViewed` | `PullRequestRef` | `PullRequestFilesViewedResult` | `PullRequestRpcError` | unary | `orchestration:read` | 1059 |
| `pullRequests.filterOptions` | `PullRequestFilterOptionsInput` | `PullRequestFilterOptions` | `PullRequestRpcError` | unary | `orchestration:read` | 975 |
| `pullRequests.generateReviewGuide` | `PullRequestRef` | `PullRequestReviewGuideResult` | `PullRequestRpcError` | unary | `orchestration:operate` | 1041 |
| `pullRequests.invalidate` | `PullRequestInvalidateInput` | `Void` | `PullRequestRpcError` | unary | `orchestration:read` | 1125 |
| `pullRequests.labelCandidates` | `PullRequestRef` | `PullRequestLabelCandidateList` | `PullRequestRpcError` | unary | `orchestration:read` | 1157 |
| `pullRequests.linkedThreads` | `PullRequestRef` | `PullRequestLinkedThreadsResult` | `PullRequestRpcError` | unary | `orchestration:read` | 1005 |
| `pullRequests.list` | `PullRequestListInput` | `PullRequestListResult` | `PullRequestRpcError` | unary | `orchestration:read` | 962 |
| `pullRequests.listStats` | `PullRequestListStatsInput` | `PullRequestListStatsResult` | `PullRequestRpcError` | unary | `orchestration:read` | 969 |
| `pullRequests.preview` | `PullRequestRef` | `PullRequestPreview` | `PullRequestRpcError` | unary | `orchestration:read` | 1017 |
| `pullRequests.replyToThread` | `PullRequestThreadReplyInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1107 |
| `pullRequests.reportState` | `PullRequestReportStateInput` | `Void` | `PullRequestRpcError` | unary | `orchestration:read` | 1131 |
| `pullRequests.requestReviewers` | `PullRequestReviewerRequestInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1151 |
| `pullRequests.reviewGuide` | `PullRequestRef` | `PullRequestReviewGuideResult` | `PullRequestRpcError` | unary | `orchestration:read` | 1035 |
| `pullRequests.reviewerCandidates` | `PullRequestRef` | `PullRequestReviewerCandidateList` | `PullRequestRpcError` | unary | `orchestration:read` | 1145 |
| `pullRequests.routing` | `PullRequestRef` | `PullRequestRoutingResult` | `PullRequestRpcError` | unary | `orchestration:read` | 981 |
| `pullRequests.routingIdentity` | `PullRequestRoutingIdentityInput` | `PullRequestRoutingIdentityResult` | `PullRequestRpcError` | unary | `orchestration:read` | 987 |
| `pullRequests.runAction` | `PullRequestActionInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1077 |
| `pullRequests.setFilesViewed` | `PullRequestSetFilesViewedInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1071 |
| `pullRequests.setLabels` | `PullRequestLabelChangeInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1163 |
| `pullRequests.setReaction` | `PullRequestReactionInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1119 |
| `pullRequests.setThreadResolution` | `PullRequestThreadResolutionInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1113 |
| `pullRequests.stack` | `PullRequestRef` | `Nullable(PullRequestStack)` | `PullRequestRpcError` | unary | `orchestration:read` | 999 |
| `pullRequests.submitReview` | `PullRequestSubmitReviewInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1101 |
| `pullRequests.subscribeRefreshes` | `Object({})` | `NonNegativeInt` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1137 |
| `pullRequests.summary` | `PullRequestRef` | `PullRequestSummary` | `PullRequestRpcError` | unary | `orchestration:read` | 993 |
| `pullRequests.threadComments` | `PullRequestThreadCommentsInput` | `PullRequestThreadCommentsResult` | `PullRequestRpcError` | unary | `orchestration:read` | 1047 |
| `pullRequests.update` | `PullRequestUpdateInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1083 |
| `pullRequests.updateComment` | `PullRequestCommentUpdateInput` | `Void` | `PullRequestRpcError` | unary | `source-control:write` | 1095 |
| `review.getDiffFileContents` | `ReviewDiffFileContentsInput` | `ReviewDiffFileContentsResult` | `Union([ReviewDiffPreviewError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1452 |
| `review.getDiffPreview` | `ReviewDiffPreviewInput` | `ReviewDiffPreviewResult` | `Union([ReviewDiffPreviewError, EnvironmentAuthorizationError])` | unary | `filesystem:read` | 1446 |
| `scheduledTasks.delete` | `ScheduledTaskDeleteInput` | `ScheduledTaskDeleteResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1841 |
| `scheduledTasks.getWebhookDelivery` | `ScheduledTaskGetWebhookDeliveryInput` | `ScheduledTaskGetWebhookDeliveryResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1882 |
| `scheduledTasks.list` | `ScheduledTaskListInput` | `ScheduledTaskListResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1816 |
| `scheduledTasks.listWebhookDeliveries` | `ScheduledTaskListWebhookDeliveriesInput` | `ScheduledTaskListWebhookDeliveriesResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1873 |
| `scheduledTasks.rotateWebhookToken` | `ScheduledTaskRotateWebhookTokenInput` | `ScheduledTaskMutationResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1853 |
| `scheduledTasks.runNow` | `ScheduledTaskRunNowInput` | `ScheduledTaskRunNowResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1847 |
| `scheduledTasks.setEnabled` | `ScheduledTaskSetEnabledInput` | `ScheduledTaskMutationResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1835 |
| `scheduledTasks.subscribe` | `ScheduledTaskListInput` | `ScheduledTaskListResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1822 |
| `scheduledTasks.upsert` | `ScheduledTaskUpsertInput` | `ScheduledTaskMutationResult` | `Union([ScheduledTaskError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1829 |
| `secrets.answerRequest` | `SecretRequestAnswerInput` | `Void` | `Union([SecretRequestError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1868 |
| `server.acceptAcpRegistryUrlAuth` | `AcpRegistryAcceptUrlAuthInput` | `AcpRegistryAcceptUrlAuthResult` | `EnvironmentAuthorizationError` | unary | `providers:manage` | 813 |
| `server.checkOtlpEndpoint` | `OtlpEndpointCheckInput` | `OtlpEndpointCheckResult` | `EnvironmentAuthorizationError` | unary | `settings:write` | 903 |
| `server.commitDesktopUpdate` | `DesktopUpdateCommitInput` | `ServerSelfUpdateResult` | `Union([ServerSelfUpdateError, EnvironmentAuthorizationError])` | unary | `environment:maintain` | 746 |
| `server.deleteAcpRegistrySession` | `AcpRegistryDeleteSessionInput` | `AcpRegistryDeleteSessionResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 831 |
| `server.disableAcpRegistryProvider` | `AcpRegistryDisableProviderInput` | `AcpRegistryDisableProviderResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 849 |
| `server.discoverSourceControl` | `Object({})` | `SourceControlDiscoveryResult` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 786 |
| `server.getBackgroundPolicy` | `Object({})` | `BackgroundPolicySnapshot` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 950 |
| `server.getConfig` | `Object({})` | `ServerConfig` | `Union([KeybindingsConfigError, ServerSettingsError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 608 |
| `server.getDatabaseStorage` | `Object({})` | `Nullable(DatabaseStorageStatus)` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 764 |
| `server.getHostResources` | `Object({})` | `HostResourcesSnapshot` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 876 |
| `server.getProcessDiagnostics` | `Object({})` | `ServerProcessDiagnosticsResult` | `EnvironmentAuthorizationError` | unary | `diagnostics:read` | 870 |
| `server.getProcessResourceHistory` | `ServerProcessResourceHistoryInput` | `ServerProcessResourceHistoryResult` | `EnvironmentAuthorizationError` | unary | `diagnostics:read` | 882 |
| `server.getResourceTelemetryHistory` | `ResourceTelemetryHistoryInput` | `ResourceTelemetryHistory` | `EnvironmentAuthorizationError` | unary | `diagnostics:read` | 888 |
| `server.getSettings` | `Object({})` | `ServerSettings` | `Union([ServerSettingsError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 771 |
| `server.getStorageCleanupReport` | `Object({})` | `Nullable(StorageCleanupReport)` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 757 |
| `server.getTraceDiagnostics` | `Object({})` | `ServerTraceDiagnosticsResult` | `EnvironmentAuthorizationError` | unary | `diagnostics:read` | 864 |
| `server.getUsageSummary` | `UsageSummaryInput` | `UsageSummary` | `Union([EnvironmentAuthorizationError, UsageReadError])` | unary | `diagnostics:read` | 909 |
| `server.importAcpRegistrySession` | `AcpRegistryImportSessionInput` | `AcpRegistryImportSessionResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 825 |
| `server.listAcpRegistryProviders` | `AcpRegistryListProvidersInput` | `AcpRegistryListProvidersResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 837 |
| `server.listAcpRegistrySessions` | `AcpRegistryListSessionsInput` | `AcpRegistryListSessionsResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 819 |
| `server.logoutAcpRegistry` | `AcpRegistryLogoutInput` | `AcpRegistryLogoutResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 858 |
| `server.prepareAcpRegistryAgent` | `AcpRegistryPrepareInput` | `AcpRegistryPrepareResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 798 |
| `server.probe` | `Object({})` | `Object({})` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 602 |
| `server.refreshProviders` | `Object({ instanceId: Optional(ProviderInstanceId), cwd: Optional(TrimmedNonEmptyString), fresh: Optional(Boolean), refreshModels: Optional(Boolean), workspaceSnapshotsOnDemand: Optional(Boolean), usageLimitsCommand: Optional(Boolean), })` | `ServerProviderUpdatedPayload` | `Union([EnvironmentAuthorizationError, ProviderSetupError])` | unary | `orchestration:read` | 614 |
| `server.refreshUsageRates` | `Object({})` | `UsagePricing` | `EnvironmentAuthorizationError` | unary | `diagnostics:read` | 915 |
| `server.removeKeybinding` | `ServerRemoveKeybindingInput` | `ServerRemoveKeybindingResult` | `Union([KeybindingsConfigError, EnvironmentAuthorizationError])` | unary | `settings:write` | 596 |
| `server.reportClientActivity` | `ClientActivityReportInput` | `Void` | `EnvironmentAuthorizationError` | unary | `orchestration:read` | 940 |
| `server.reportHostPowerState` | `HostPowerSnapshot` | `Void` | `EnvironmentAuthorizationError` | unary | `environment:maintain` | 945 |
| `server.retryResourceTelemetry` | `Object({})` | `ResourceTelemetryRetryResult` | `EnvironmentAuthorizationError` | unary | `diagnostics:read + dynamic rules` | 897 |
| `server.runStorageCleanup` | `Object({})` | `StorageCleanupReport` | `Union([ServerSettingsError, EnvironmentAuthorizationError])` | unary | `settings:write` | 752 |
| `server.searchAcpRegistry` | `AcpRegistrySearchInput` | `AcpRegistrySearchResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 792 |
| `server.setAcpRegistryProvider` | `AcpRegistrySetProviderInput` | `AcpRegistrySetProviderResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 843 |
| `server.signalProcess` | `ServerSignalProcessInput` | `ServerSignalProcessResult` | `EnvironmentAuthorizationError` | unary | `environment:maintain` | 921 |
| `server.uninstallAcpRegistryManagedBinary` | `AcpRegistryManagedBinaryUninstallInput` | `AcpRegistryManagedBinaryUninstallResult` | `Union([AcpRegistryOperationError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 804 |
| `server.updateProvider` | `ServerProviderUpdateInput` | `ServerProviderUpdatedPayload` | `Union([ServerProviderUpdateError, EnvironmentAuthorizationError])` | unary | `providers:manage` | 634 |
| `server.updateServer` | `ServerSelfUpdateInput` | `ServerSelfUpdateResult` | `Union([ServerSelfUpdateError, EnvironmentAuthorizationError])` | unary | `environment:maintain` | 733 |
| `server.updateServerWithProgress` | `ServerSelfUpdateInput` | `ServerSelfUpdateProgressEvent` | `Union([ServerSelfUpdateError, EnvironmentAuthorizationError])` | stream | `environment:maintain` | 739 |
| `server.updateSettings` | `Object({ patch: ServerSettingsPatch, providerInstanceMutation: Optional(ProviderInstanceMutation), })` | `ServerSettings` | `Union([ServerSettingsError, EnvironmentAuthorizationError])` | unary | `settings:write + dynamic rules` | 777 |
| `server.upsertKeybinding` | `ServerUpsertKeybindingInput` | `ServerUpsertKeybindingResult` | `Union([KeybindingsConfigError, EnvironmentAuthorizationError])` | unary | `settings:write` | 590 |
| `shell.openInEditor` | `LaunchEditorInput` | `Void` | `Union([ExternalLauncherError, EnvironmentAuthorizationError])` | unary | `orchestration:operate` | 1271 |
| `sourceControl.cloneRepository` | `SourceControlCloneRepositoryInput` | `SourceControlCloneRepositoryResult` | `Union([SourceControlRepositoryError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1175 |
| `sourceControl.lookupRepository` | `SourceControlRepositoryLookupInput` | `SourceControlRepositoryInfo` | `Union([SourceControlRepositoryError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1169 |
| `sourceControl.publishRepository` | `SourceControlPublishRepositoryInput` | `SourceControlPublishRepositoryResult` | `Union([SourceControlRepositoryError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1211 |
| `subscribeAuthAccess` | `Object({})` | `AuthAccessStreamEvent` | `Union([AuthAccessStreamError, EnvironmentAuthorizationError])` | stream | `access:read` | 1891 |
| `subscribeBackgroundPolicy` | `Object({})` | `BackgroundPolicySnapshot` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1898 |
| `subscribeDeviceState` | `Object({})` | `DeviceServiceState` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1627 |
| `subscribeDiscoveredLocalServers` | `Object({ configuredUrls: Optional(ConfiguredLocalServerUrls), })` | `DiscoveredLocalServerList` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1566 |
| `subscribePreviewEvents` | `Object({})` | `PreviewEvent` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1559 |
| `subscribeProjectClones` | `ProjectCloneSubscribeInput` | `ProjectCloneListEvent` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1204 |
| `subscribeResourceTelemetry` | `Object({})` | `ResourceTelemetrySnapshot` | `EnvironmentAuthorizationError` | stream | `diagnostics:read` | 1905 |
| `subscribeServerConfig` | `Object({ environmentThemes: Optional(Boolean), usageLimitSources: Optional(Boolean), usageLimitsCommand: Optional(Boolean), workspaceSnapshotsOnDemand: Optional(Boolean), })` | `ServerConfigStreamEvent` | `Union([KeybindingsConfigError, ServerSettingsError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1794 |
| `subscribeServerLifecycle` | `Object({})` | `ServerLifecycleStreamEvent` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1809 |
| `subscribeTerminalEvents` | `Object({})` | `TerminalEvent` | `EnvironmentAuthorizationError` | stream | `terminal:read` | 1780 |
| `subscribeTerminalMetadata` | `Object({})` | `TerminalMetadataStreamEvent` | `EnvironmentAuthorizationError` | stream | `terminal:read` | 1787 |
| `subscribeVcsStatus` | `VcsStatusSubscriptionInput` | `VcsStatusStreamEvent` | `Union([GitManagerServiceError, EnvironmentAuthorizationError])` | stream | `orchestration:read` | 1357 |
| `subscribeWorktreeSetup` | `WorktreeSetupSubscribeInput` | `WorktreeSetupStreamEvent` | `EnvironmentAuthorizationError` | stream | `orchestration:read` | 1379 |
| `terminal.attach` | `TerminalAttachInput` | `TerminalAttachStreamEvent` | `Union([TerminalError, EnvironmentAuthorizationError])` | stream | `terminal:operate` | 1464 |
| `terminal.clear` | `TerminalClearInput` | `Void` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1488 |
| `terminal.close` | `TerminalCloseInput` | `Void` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1499 |
| `terminal.observe` | `TerminalObserveInput` | `TerminalAttachStreamEvent` | `Union([TerminalError, EnvironmentAuthorizationError])` | stream | `terminal:read` | 1471 |
| `terminal.open` | `TerminalOpenInput` | `TerminalSessionSnapshot` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1458 |
| `terminal.resize` | `TerminalResizeInput` | `Void` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1483 |
| `terminal.restart` | `TerminalRestartInput` | `TerminalSessionSnapshot` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1493 |
| `terminal.write` | `TerminalWriteInput` | `Void` | `Union([TerminalError, EnvironmentAuthorizationError])` | unary | `terminal:operate` | 1478 |
| `vcs.createRef` | `VcsCreateRefInput` | `VcsCreateRefResult` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1429 |
| `vcs.createWorktree` | `Object({ ...VcsCreateWorktreeInput.fields, path: Null })` | `VcsCreateWorktreeResult` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1417 |
| `vcs.init` | `VcsInitInput` | `Void` | `Union([VcsError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1441 |
| `vcs.listRefs` | `VcsListRefsInput` | `VcsListRefsResult` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1411 |
| `vcs.pull` | `VcsPullInput` | `VcsPullResult` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1364 |
| `vcs.refreshStatus` | `Object({ ...VcsStatusInput.fields, localOnly: Optional(Boolean), })` | `VcsStatusResult` | `Union([GitManagerServiceError, EnvironmentAuthorizationError])` | unary | `orchestration:read` | 1370 |
| `vcs.removeWorktree` | `VcsRemoveWorktreeInput` | `Void` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1424 |
| `vcs.switchRef` | `VcsSwitchRefInput` | `VcsSwitchRefResult` | `Union([GitCommandError, EnvironmentAuthorizationError])` | unary | `source-control:write` | 1435 |
| `worktreeSetup.cancel` | `WorktreeSetupCancelInput` | `WorktreeSetupCancelResult` | `EnvironmentAuthorizationError` | unary | `orchestration:operate` | 1386 |

Payload-dependent enforcement (all required permissions must be present):

- `assets.createUrl`: filesystem:read for workspace-file/media-file/draft-workspace-file; orchestration:read otherwise.
- `server.retryResourceTelemetry`: environment:maintain **and** diagnostics:read.
- `server.updateSettings`: evaluate the patch's owned settings domains; provider instance mutation additionally requires providers:manage; if patch is empty and only mutation is supplied, only providers:manage is required.
- `projects.mutate`: orchestration:operate; add settings:write exactly when a non-delete mutation supplies scripts.
- `git.preparePullRequestThread`: source-control:write; add orchestration:operate for worktree mode with threadId.
- `git.runStackedAction`: source-control:write; add orchestration:read when threadId is supplied and action is create_pr or commit_push_pr.
- `device.list`: orchestration:operate when updateTool or retryHostId is requested; otherwise orchestration:read.

Sources: `apps/server/src/auth/RpcAuthorization.ts:240`, `:261`, `:322`; `packages/contracts/src/clientRpcPermissions.ts:67`. Specific patch/property rules are included in Appendix E. Grant checking is exact membership; “operate” does not automatically imply read or terminal/filesystem access.

The method constants keep retired names such as projects.list/add/remove. Only declarations in the matrix belong to the served WsRpcGroup. Do not implement retired tags merely because they occur in WS_METHODS. The matrix is cross-checked against `lib:tests/fixtures/t3-contract.json` and `lib:tools/extract-t3-contract`.

## 5. Orchestration: commands, events, projections, replay

### 5.1 Write path and identity

`orchestration.dispatchCommand` payload is the command object itself, a discriminated union on `type`. It is **not** `{command:<...>}`. All command variants are transcribed in Appendix C's OrchestrationV2Command. Each carries the variant's commandId, thread identifiers, timestamps and data. RPC success is `{sequence:nonnegative integer}`. The accepted command result does not promise that an agent turn has completed. Provider execution arrives later via events.

The practical decider is the command planner inside Orchestrator.ts (`dispatchOnce` and its command-specific services), not an RPC transport handler. Under concurrency control it reads projections, validates command/state/mode preconditions, plans domain events and pending effects, and commits events, projection changes, receipt and effects together. Publish only **after** commit. Execute provider/process effects outside the DB transaction; their observations feed the same event/projection path. Recover pending work after restart rather than re-sending every provider turn. Source: `apps/server/src/orchestration-v2/Orchestrator.ts:12101`, `:12177`, `:12263`; EventSink source index below.

CommandId is idempotency identity. A repeated accepted command returns its original result sequence with no repeated mutation; a previous rejection stays rejected; reusing a commandId for an incompatible thread produces a conflict. Some stop/settle operations validly produce no new events and still record receipts. queue.resume and queued-run.start have deliberate replay handling for starting queued work. A receipt is not permission to accept a new arbitrary payload. Source: `apps/server/src/orchestration-v2/Orchestrator.ts:12110`, `:12121`, `:12151`, `:12231`.

Families: thread.create/fork/metadata/modes/model-selection drive thread and provider-thread projections; message.dispatch starts/queues/steers a run; runtime-request.respond resolves approval/question state; run.interrupt/thread.stop/subagent.stop cancel owned work; checkpoint.rollback restores workspace and records state changes; queue and queued-run commands change durable ordering; archive/settle/snooze/pin/visit affect shell visibility/order; delegated_task commands govern child execution/completion delivery. Guest share opening/redemption/submission are server-only operations; owners can close a share, dismiss a message or send an approved guest message with message.dispatch.guestMessageId. Do not expose the larger server-internal command union as the public RPC union.

### 5.2 Events and projections

Stored events have a global sequence and a domain event with `id` (EventId), `type`, `threadId`, `occurredAt`, optional causal/provider/run/node metadata and variant-specific payload. Consult the dictionary's OrchestrationV2DomainEvent/StoredEvent for exact fields rather than assuming every variant has an identical payload. Projections are current materialized rows, not raw provider messages. Preserve stable IDs across all links: app thread/run/node IDs differ from native provider session/thread/turn/item IDs.

Reducers update `thread`, runs, attempts, nodes, provider sessions/threads/turns, runtime requests, messages, turn items, visible inherited/local items, plans, queue, checkpoint tree, subagents and effects. Whole updated records generally replace an existing record by ID; deletion/removal events must remove it; aggregate shell records have their own shapes. The event dictionary includes every event variant, not just the chat-message subset. The client reducer is an additional executable reference: `packages/client-runtime/src/state/orchestrationV2Projection.ts:1`; Rust `lib:src/projection.rs:1`.

`sequence` is the shared durable log position. `snapshotSequence` is a consistent high-water mark for the snapshot. `afterSequence` is exclusive. Do **not** renumber after filtering or require contiguous sequences on a thread stream. Thread sequences skip project/other-thread events; shell sequences skip irrelevant/coalesced events. Clients deduplicate replay/live overlap and replace their state on any snapshot, even after requesting a resume.


The stored wrapper is exactly `{sequence,commandId:string|null,event}`. The domain event's common fields are `{id,threadId,runId?,nodeId?,driver?,providerInstanceId?,rawEventId?,occurredAt}` plus `type` and its payload. Stream events deliberately omit stored commandId: `{kind:"event",sequence,event}`. `id` is the event ID; do not rename it to eventId. Source: `packages/contracts/src/orchestrationV2.ts:1709`, `:2115`.

Representative effects of command planning (not a fixed event count; exact sequences depend on current state):

| Command family | Durable event / projection result |
|---|---|
| thread.create | thread.created with full AppThread; initial provider-thread.updated when planned |
| thread.archive/unarchive/delete | thread.archived/unarchived/deleted with updated AppThread; deletion also plans owned-resource cleanup |
| thread.settle/unsettle/snooze/unsnooze/pin/unpin/reorder/visit | matching thread mutation event with whole AppThread payload |
| thread.metadata.update / modes / model / provider.switch | thread.metadata-updated/runtime-mode-updated/interaction-mode-updated/model-selection-updated/provider-switched; provider/session effects when required |
| message.dispatch | message and turn-item records, run.created or queued state, provider execution effect; steering/continuations differ from a fresh run |
| runtime-request.respond | resolves runtime request and plans native provider response; turn need not finish |
| checkpoint.rollback | checkpoint.rollback-requested then checkpoint/restore result; failure remains observable |
| queue controls | durable queued-run/queue state and optional start-next effect |
| guest owner actions | thread.guest-share-updated; sending a guest message consumes its pending state and preserves attribution |

Sources: `apps/server/src/orchestration-v2/Orchestrator.ts:2558`, `:4983`, `:10801`, `:11745`; actual transactional projection application: `apps/server/src/orchestration-v2/EventSink.ts:251`, `:275`, `:353`, `:650`. These families explain the write model; the complete variant schemas remain the normative field inventory below.

### 5.3 Shell stream

`orchestration.subscribeShell` input `{afterSequence?,requestCompletionMarker?,omitIdleSubagents?,omitOlderSettled?}`. First connection sends `{kind:"snapshot",snapshot:{schemaVersion:2,snapshotSequence,projects,threads,archivedThreads,...}}`, optional enrichment items, optionally `{kind:"synchronized"}`, then live shell updates. Live items include thread.updated/thread.removed/project.updated/project.removed and enrichment variants; their exact fields are in OrchestrationV2ShellStreamItem. The shell is a summary, **not** a complete thread transcript. Active and archived shell subscriptions are separate. `orchestration.subscribeArchivedShell` loads the archived snapshot then tails relevant changes.

On resume, bound the high-water mark, replay stored events in `(afterSequence,highWater]`, refresh project metadata/enrichment, then tail after highWater. If the cursor is ahead, too far behind or exceeds byte budget, send a replacement snapshot. Subscribe/buffer before snapshot/replay so concurrent changes cannot be lost. Current shell resume maximum gap is 1000 global sequence positions and stored payload budget is 8 MiB (`apps/server/src/ws.ts:579`). Projection schema version is **2** (`apps/server/src/orchestration-v2/ProjectionStore.ts:654`; see verified index below if line shifts). Shell events are coalesced in windows (up to 512 events / 50 ms); unchanged thread summaries can be skipped. Enrichment updates must not precede the initial frame. Source: `apps/server/src/ws.ts:730`, `:991`, `:1087`, `:1140`, `:1170`.

Omission flags only apply when advertised by `shellThreadFamilies` / `shellOlderSettled` server capabilities. Restore omitted family rows with getThreadFamily; restore older settled rows/counts with getOlderSettledThreads. Family returns root, forks and all active subagent descendants sharing lineage.rootThreadId, independent of run state. Older-settled paging sorts settledAt descending then ID ascending; missing cursor ID means strict before time, limit 0 returns counts, limit is 0..50. Include omitted children when resolving worktree occupants (getWorktreeThreadIds). Sources: `packages/contracts/src/orchestrationV2.ts:3434`, `:3454`, `:3501`.

### 5.4 Thread detail, history and stream

`orchestration.subscribeThread` input `{threadId,afterSequence?,requestCompletionMarker?,acceptBoundedSnapshot?,acceptCompactTurnItems?,acceptDedupedBodies?,acceptCompactIds?}`. Snapshot item `{kind:"snapshot",snapshotSequence,projection,historyCursor?,hasMoreHistory?,latestLocalTurnOrdinal?,turnItemsOmitLocalVisible?,ids?}`; event item `{kind:"event",sequence,event}`; completion marker `{kind:"synchronized"}` only when requested. Completion marker means caught up to the initial boundary, **not** end of stream or end of run.

No cursor → snapshot then live. Resume measures the thread high-water and replay costs; cursor ahead, too much replay, unsafe stored/projected bytes, or replay including a retained creation event for an existing thread can force a snapshot. Unknown newer event types are skipped on persisted replay without losing later events; compatible clients still advance their cursor for unknown live events. Subscribe before the load to retain concurrent publications. Sources: `apps/server/src/ws.ts:791`, `:838`, `:874`, `:911`; replay policy source index below.

HTTP `/threads/:id` returns a detail snapshot. The bounded route returns `{snapshotSequence,projection,historyCursor:string|null,hasMoreHistory:boolean,latestLocalTurnOrdinal:number|null,...opt-ins}`. `/history` returns `{snapshotSequence,items,nextCursor:string|null,hasMoreHistory,ids?}`. Keep cursor opaque; it is history paging state, **not** the live event cursor. Resume live events from snapshotSequence. History view conversation omits command_execution/reasoning/file_change/dynamic_tool activity; activity view retains it. Current page policy: 10 user turns, 75 generic items, 500 turn items, 1 MiB encoded budget; older-thread policy can use 20 turns, raw turn limit 150. Oversized single records need the dedicated item read. Source: `apps/server/src/orchestration-v2/threadHistoryPaging.ts:9`, `:15`, `:24`; `packages/contracts/src/orchestrationV2.ts:3550`.

### 5.5 Compact transport opt-ins (never apply globally)

- compactTurnItems / acceptCompactTurnItems: snapshot turnItems can omit the local visible prefix and set `turnItemsOmitLocalVisible:true`. Reconstitute local visible item records before giving the projection to reducers.
- dedupeBodies / acceptDedupedBodies: repeated user/assistant message text, checkpoint files and subagent prompt/result may be omitted with `textOmitted`, `filesOmitted`, `promptOmitted`, `resultOmitted`. Restore from authoritative same-ID records. Snapshot bodies must be restored before local visible rows. One live deduper spans snapshot/replay/live, never an independent per-chunk cache. If a referenced body is missing, resnapshot without advancing past lost content.
- compactIds / acceptCompactIds: add `ids:[...]` and substitute numeric indices **only** at record-level `id`, `*Id`, elements of `*Ids`, and nested record `item`. Do not recursively rewrite arbitrary tool JSON, user text, command arguments or opaque data. Expand and remove the table at the transport boundary. A numeric ID index is not the underlying string ID. Internal `idTable:true` is a codec marker, not the client opt-in field.

These encodings apply to bounded/history snapshots and the declared thread-stream codec, not every JSON route. Source: `packages/contracts/src/orchestrationV2.ts:3544`, `:3561`, `:3642`; `lib:src/wire.rs:5`, `:29`, `:55`; `lib:docs/NIGHTLY_3726.md:6`.

Command-output subscriptions are separate from the thread transcript. They deliver replace/append/control frames for a requested thread/item output and must obey their own replacement/truncation/offset semantics. Do not pretend transient command output is a normal message.updated event. Source: OrchestrationV2CommandOutputFrame in Appendix C; `packages/contracts/src/orchestrationV2.ts:3339`.

## 6. Provider runtime protocols

### Codex

Spawn the configured executable with `app-server` plus tokenized launch args; `T3CODE_CODEX_LAUNCH_ARGS` overrides stored launch args (`apps/server/src/provider/codexLaunchArgs.ts:3`). The transport is bidirectional app-server JSON-RPC over stdio, separate from Effect RPC. Initialize once per shared process: request initialize, then notification initialized. Start/resume native threads; turn/start sends prompt/model/approval/sandbox settings, turn/steer injects input into the active turn, turn/interrupt cancels it. The server must also answer provider-originated approvals/elicitation requests by their native request IDs. Preserve native and app ID mappings and provider-session bindings through restart. Sources: `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts:78`, `:1684`, `:6321`, `:6587`, `:6783`; app-server client schema/protocol sources indexed below.

Notification mapping: turn/started creates/starts provider turn/run-attempt nodes; text/reasoning deltas update message/turn-item bodies; item start/completion creates/finalizes command/file/tool/plan items; native requests become runtime-request projections; usage/rate-limit notifications update usage and provider status; terminal completion/error/interruption finalizes run state only after owned background/delegated work is settled. Preserve causal node/turn identity; do not collapse all tool traffic into assistant text. Full domain record schema is provider-neutral.

### Claude: Node SDK versus a non-Node implementation

The lockfile pins Claude Agent SDK **0.3.276** (`pnpm-lock.yaml:829`). Upstream imports `@anthropic-ai/claude-agent-sdk` and calls `query({prompt:<async iterable SDKUserMessage>,options})`. It retains a query runtime and prompt queue, registers MCP servers through setMcpServers over stdin, and manages the spawned Claude process group. This is a **Node library**, not a network server a Rust executable can call directly. Source: `apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts:48`, `:576`, `:921`, `:938`, `:1000`.

A non-Node server has two choices: ship a Node subprocess shim using the pinned SDK (closest parity), or implement the subprocess protocol that SDK drives. For the direct path, launch the installed Claude Code executable in print mode with `--input-format stream-json --output-format stream-json --verbose`, keep stdin/stdout open, and enable partial messages for deltas (`--include-partial-messages`). Use the configured cwd/environment, model, system prompt and permission mode; resume the native session with `--resume <session-id>` when required. Send newline-delimited user objects such as:

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Hello"}]},"parent_tool_use_id":null,"session_id":""}
```

Parse `system` (including init), `stream_event`, `assistant`, `user` tool results, `result`, and control_request/control_response messages; never parse stderr as protocol. Retain session_id/native message IDs/tool_use IDs and parent_tool_use_id. Handle the SDK control handshake, permission callbacks, interrupt/model/mode/MCP controls, rather than repeatedly spawning one-shot `claude -p` for every message. Permission requests must surface as runtime requests and remain pending until the client responds. A typical permission response envelope is `{type:"control_response",response:{subtype:"success",request_id:<native request id>,response:{behavior:"allow",updatedInput:<tool input>}}}`; deny responses use the appropriate deny behavior/message.

**Evidence limit:** those CLI flags and control-envelope details describe the SDK-compatible subprocess approach; this checkout exposes SDK types/calls but does not vendor the complete installed Anthropic SDK/CLI implementation. Exact initialize/control variants, flags supported by a given CLI build, fork/resume-at semantics, MCP control and background task lifetimes must be pinned against that installed SDK/CLI before claiming direct-adapter parity. A Node shim is the immediately evidenced path. Do not label the TS adapter itself a direct stream-json implementation.

Mapping: stream_event assistant text/thinking deltas → message/turn-item updates; assistant tool_use and user tool_result → tool activity lifecycle; Task/Agent calls and parent IDs → subagent nodes; result usage/error/stop → terminal provider-turn state. Background work can outlive a result and block query replacement; upstream expressly detects this (`ClaudeAdapterV2.ts:546`). Task completion must not be inferred from a resume tool-result acknowledgement.

### OpenCode and ACP providers

OpenCode is not driven as Codex app-server. OpenCode runtime launches `opencode serve --hostname=<host> --port=<port>` or connects to configured serverUrl/password; reuse process ownership and authenticate to its HTTP service. Source: `packages/provider-opencode/src/server/OpenCodeRuntime.ts:680`; v2 connection `packages/provider-opencode/src/server/v2/OpenCode2Server.ts:112`. OpenCode 2 uses its typed HTTP API and SSE `/api/event` with Accept text/event-stream; subscribe before sending work because events are volatile. session.execution.started/succeeded/failed/interrupted supply execution boundaries even when unfamiliar frames must be tolerated. Source: `packages/provider-opencode/src/server/v2/OpenCode2Client.ts:32`, `:53`, `:117`. Version-1 and version-2 OpenCode APIs differ; do not assume their paths/schemas are interchangeable.

ACP registry and ACP-based drivers spawn their configured command/args/cwd/env and use ACP's bidirectional JSON-RPC over stdio: initialize (capabilities/auth methods), session/new or advertised session/load/session/resume, session/prompt, session/update notifications, session/request_permission, cancel and supported mode/model/config calls. Filesystem/terminal callbacks are server responsibilities. Drivers such as Cursor/Muse/Grok/Pi have separate runtime implementations; Pi also has a native RPC mode. Do not run a shell for arbitrary provider strings when a configured argv is available. Source: `packages/provider-acp/src/server/AcpSessionRuntime.ts:1367`, `:1443`, `:1770`, `:2040`, `:2165`; `packages/provider-pi/src/server/rpc.ts:1`.

Provider adapters normalize native records into the same event schema; clients do not consume native Codex/Claude/ACP/OpenCode frames directly. Native IDs, capability probes, model selection, usage accounting, secrets and lifecycle behavior remain adapter-specific. Feature advertisement must reflect the subset actually implemented.

## 7. PTY terminals, VCS/checkpoints, settings and secrets

Terminal RPC exact schemas are in Appendix C. terminal.open returns TerminalSessionSnapshot; terminal.attach is an operating subscription while terminal.observe is a read-only subscription. write/resize/clear/restart/close are unary controls. The PTY manager owns cwd, shell/env, dimensions, scrollback, lifecycle and sequences; subscriptions must attach without dropping output between the snapshot and live tail. UTF-8 output can split across PTY reads, so decode incrementally. Do not execute terminal.write data through a separate shell command. Restart starts a new process lifecycle and snapshot; close must terminate owned process groups. Terminal streams carry their own sequence, not the orchestration log cursor. Metadata subscriptions are distinct from terminal byte output. Source: `packages/contracts/src/terminal.ts:1`; `apps/server/src/terminal/Manager.ts:1`; `apps/server/src/terminal/NodePtyAdapter.ts:1`; `apps/server/src/terminal/OutputProtocol.ts:1`; common PTY interface `packages/shared/src/PtyAdapter.ts:1`.

VCS RPCs include status streams, list/create/switch refs, worktree create/remove, pull/init and stacked actions. Git action progress is a stream, not a unary collected result. Worktree setup has a separate cancellable stream. Source-control operations that mutate remote/local refs require source-control:write as listed in the matrix. Status and orchestration are distinct projections. Use subprocess argv, preserve cwd, serialize conflicting repository mutations, and distinguish Git command failure from manager/service failure. Pull request operations have host-specific adapters and typed unavailable errors; absence of gh or a configured host is not successful empty data. Source: `packages/contracts/src/git.ts:1`; `packages/contracts/src/pullRequest.ts:1`; handlers `apps/server/src/ws.ts:2620` onward.

Checkpointing is a VCS-driver interface: captureCheckpoint(cwd, checkpointRef), restoreCheckpoint with optional HEAD fallback, hasCheckpointRef, diffCheckpoints with patch/numstat, whitespace and path filters, listAuthoredPaths, deleteCheckpointRefs. The app models nested scopes and checkpoint ancestry; rollback is a command and must coordinate provider/run state with workspace restoration. Filesystem content is stored in Git objects/refs by the Git driver, not solely in the SQL projection. A DB backup alone cannot restore all workspace checkpoints. Sources: `apps/server/src/checkpointing/CheckpointStore.ts:10`, `:45`, `:78`; `apps/server/src/vcs/GitVcsDriver.ts:1` and source index below.

settings.json is schema-validated JSON, patched through server.updateSettings; project overrides are separate from environment defaults. Provider instances have identity/driver/options and mutation semantics; settings must round-trip unknown supported driver-specific configuration rather than assume a single global Codex process. Consult ServerSettings/ServerSettingsPatch and nested defaults in Appendix C. Writes are atomic and config subscriptions notify all clients. keybindings.json is a separate validated configuration; keybindings RPCs update it, not a settings subfield. Source: `apps/server/src/serverSettings.ts:1`; `packages/contracts/src/settings.ts:1`; `apps/server/src/keybindings.ts:1`.

Secrets use raw byte files `stateDir/secrets/<name>.bin`, directory mode 0700 and files 0600, atomic replacement/exclusive create. They are not plaintext values to echo back from settings. Signing secret names include server-signing-key and asset-access-signing-key; DPoP replay markers use dpop-proof-<hash>. Sensitive settings are split into secret storage and represented in returned/persisted settings by six bullet characters `••••••`; preserving that sentinel means “keep the stored value”, not overwrite with bullets. Provider/host secret names encode identity components using UTF-8 base64url. Source: `apps/server/src/auth/ServerSecretStore.ts:154`, `:171`, `:202`, `:243`; `apps/server/src/auth/SessionStore.ts:431`; `apps/server/src/settingsSecrets.ts:25`, `:40`.

secrets.answerRequest is not a general settings secret setter: it answers a specific pending secret request with the declared delivery/answer contract. A non-Node implementation must reproduce delivery/ref semantics and keep secret bytes out of chat events and ordinary diagnostics. Exact request/answer/delivery types are included below.

## Appendix A. Public command discriminants

The following public command types are the public command union at this commit. Their complete object schemas are under OrchestrationV2Command in Appendix C. Server-internal commands are intentionally excluded.

```text
checkpoint.rollback
delegated_task.completion-delivery.acknowledge
delegated_task.completion-delivery.dispose
delegated_task.request
delegated_task.wake-policy
message.dispatch
notification.delivery.accept
prepared-run.fail
prepared-run.progress
prepared-run.release
prepared-run.retry
provider-session.detach
provider.switch
queue.resume
queued-message.promote-to-steer
queued-run.cancel
queued-run.edit
queued-run.reorder
queued-run.start
run.interrupt
runtime-request.respond
subagent.stop
thread.active.reorder
thread.archive
thread.auto-settle
thread.auto-settle.set
thread.create
thread.created.record
thread.delete
thread.fork
thread.guest-message.dismiss
thread.guest-share.close
thread.interaction-mode.set
thread.mark-unread
thread.merge_back
thread.metadata.update
thread.model-selection.set
thread.pin
thread.pin.reorder
thread.pull-request-link.sync
thread.pull-request.link
thread.pull-request.sync
thread.pull-request.unlink
thread.pull-request.watch
thread.runtime-mode.set
thread.settle
thread.snooze
thread.stop
thread.title.regeneration.complete
thread.unarchive
thread.unpin
thread.unsettle
thread.unsnooze
thread.user-input.dismiss
thread.visit
```

## Appendix B. How to read the wire dictionary

The dictionary below is **declarative schema algebra**, not executable TypeScript. Names resolve to the named entry in this dictionary; every entry has its original file/line. No imports or Effect runtime are required to implement these records.

| Form | JSON meaning |
|---|---|
| `Object({key:T,...})` | object with required fields unless T is Optional; spread `...X.fields` copies X's fields |
| `TaggedObject("tag", {...})` | object with additional `_tag:"tag"` |
| `Const(x)` | exact literal x |
| `Enum([a,b,...])` | one of these literal values |
| `Union([A,B,...])` | any one variant; usually discriminate by type, kind or _tag |
| `Optional(T)` | property may be absent; does not imply null |
| `Nullable(T)` | T or JSON null |
| `Array(T)` / `NonEmptyArray(T)` | array of T / at least one element |
| `Record(K,V)` | JSON object whose string keys satisfy K and values satisfy V |
| `String`, `Boolean`, `Number`, `Int` | JSON primitives; Int must be integral |
| `Unknown`, `Json` | arbitrary JSON; never recursively reinterpret IDs in it |
| `Never` | no valid value |
| `Void`, `Undefined` | no value; omit undefined object property on JSON encode |
| `DateTimeUtc`, `DateTimeUtcFromString`, `IsoDateTime` | ISO UTC timestamp string on wire |
| `DurationFromMillis` | number of milliseconds |
| `.check(isMaxLength(n),isBetween(...),...)` | validation predicate; numeric ranges are inclusive unless explicitly greater/less-than |
| `[decode-default: x]` | missing decoded fields may get default x; not a new JSON property |
| `...X.fields` | merge object field declarations from X |
| `Ref(X)` | recursive reference to X |
| `X.members` | union variants from X |
| `X.literals` | literal alternatives of X |

Branded IDs remain strings; branding adds no JSON wrapper. Trimmed strings validate/normalize whitespace as declared. Known tagged unions must decode known variants completely; forward-compatible unions/arrays tolerate or drop unknown variants. `ForwardCompatibleOptional(T)` is Optional(T), ignoring unknown enum values; `ForwardCompatibleArray(T)` is Array(T) filtered to recognized values; `ForwardCompatibleUnionArray([...],tag)` is a tagged array allowing newer variants to be skipped. `ForwardCompatibleUnion` is the corresponding tagged union. These are decode compatibility policies, not extra wire tags.

`projectedTurnItemArray(A,B)` has the wire shape Array(A); B describes tolerant client decoding. `withRecordIdTable(T,selector)` has the wire shape T with optional `ids` and the field substitutions specified in section 5.5. `idTable:true` is stripped by its encoder. Selectors/lambdas in these helper arguments select a container; they do not serialize. `mapFields`/`mapMembers` describe deriving a record/union, not JSON properties. `secretSetting(T)` has wire type T and marks a settings field for redaction. `asFormUrlEncoded` changes HTTP media type, not field names. Schema codec transformations are described separately in sections 2 and 5; the dictionary shows the **input/encoded side** of decoder pipes. Most predicates preserve shape. Predicate labels describe additional semantic validation; they are not serialized. `MergeFields(Base,{...})` means retain all Base fields except the fields explicitly overridden. Decode-default annotations/default constants are implementation policy, not extra wire fields.

## Appendix C. Inline wire type dictionary

### ACP_REGISTRY_MAX_AGENTS

Source: `packages/contracts/src/acpRegistry.ts:50`.

```text
512
```

### ASSET_PATH_MAX_LENGTH

Source: `packages/contracts/src/assets.ts:12`.

```text
1024
```

### AcpRegistryAcceptUrlAuthInput

Source: `packages/contracts/src/acpRegistry.ts:131`.

```text
Object({
  instanceId: ProviderInstanceId,
  elicitationId: AcpRegistryProbeText,
})
```

### AcpRegistryAcceptUrlAuthResult

Source: `packages/contracts/src/acpRegistry.ts:137`.

```text
Object({
  accepted: Boolean,
})
```

### AcpRegistryAgentId

Source: `packages/contracts/src/acpRegistry.ts:8`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(128),
  isPattern(/^[a-z0-9][a-z0-9._-]*$/),
)
```

### AcpRegistryConfigurableProvider

Source: `packages/contracts/src/acpRegistry.ts:201`.

```text
Object({
  providerId: AcpRegistryProviderId,
  supported: Array(AcpRegistryProviderProtocol).check(isMaxLength(16)),
  required: Boolean,
  current: Nullable(
    Object({
      apiType: AcpRegistryProviderProtocol,
      baseUrl: AcpRegistryUrl,
    }),
})
```

### AcpRegistryDeleteSessionInput

Source: `packages/contracts/src/acpRegistry.ts:188`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
  sessionId: AcpRegistrySessionId,
})
```

### AcpRegistryDeleteSessionResult

Source: `packages/contracts/src/acpRegistry.ts:195`.

```text
Object({ deleted: Const(true) })
```

### AcpRegistryDisableProviderInput

Source: `packages/contracts/src/acpRegistry.ts:244`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
  providerId: AcpRegistryProviderId,
})
```

### AcpRegistryDisableProviderResult

Source: `packages/contracts/src/acpRegistry.ts:251`.

```text
Object({ disabled: Const(true) })
```

### AcpRegistryDistribution

Source: `packages/contracts/src/acpRegistry.ts:44`.

```text
Enum(["binary", "npx", "uvx"])
```

### AcpRegistryImportSessionInput

Source: `packages/contracts/src/acpRegistry.ts:171`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
  sessionId: AcpRegistrySessionId,
  title: Optional(Nullable(TrimmedNonEmptyString.check(isMaxLength(1_024)))),
  updatedAt: Optional(
    Nullable(TrimmedNonEmptyString.check(isMaxLength(128))),
  ),
})
```

### AcpRegistryImportSessionResult

Source: `packages/contracts/src/acpRegistry.ts:182`.

```text
Object({
  threadId: ThreadId,
  imported: Boolean,
})
```

### AcpRegistryIntegrity

Source: `packages/contracts/src/acpRegistry.ts:47`.

```text
Enum(["sha256", "registry"])
```

### AcpRegistryListProvidersInput

Source: `packages/contracts/src/acpRegistry.ts:214`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
})
```

### AcpRegistryListProvidersResult

Source: `packages/contracts/src/acpRegistry.ts:220`.

```text
Object({
  providers: Array(AcpRegistryConfigurableProvider).check(isMaxLength(64)),
})
```

### AcpRegistryListSessionsInput

Source: `packages/contracts/src/acpRegistry.ts:155`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
  cursor: Optional(String.check(isMaxLength(2_048))),
})
```

### AcpRegistryListSessionsResult

Source: `packages/contracts/src/acpRegistry.ts:162`.

```text
Object({
  sessions: Array(AcpRegistrySession).check(isMaxLength(256)),
  nextCursor: Nullable(String.check(isMaxLength(2_048))),
  canLoad: Boolean,
  canResume: Boolean,
  canDelete: Boolean,
})
```

### AcpRegistryLogoutInput

Source: `packages/contracts/src/acpRegistry.ts:254`.

```text
Object({
  instanceId: ProviderInstanceId,
})
```

### AcpRegistryLogoutResult

Source: `packages/contracts/src/acpRegistry.ts:259`.

```text
Object({
  loggedOut: Const(true),
})
```

### AcpRegistryManagedBinaryUninstallInput

Source: `packages/contracts/src/acpRegistry.ts:95`.

```text
Object({
  agentId: AcpRegistryAgentId,
})
```

### AcpRegistryManagedBinaryUninstallResult

Source: `packages/contracts/src/acpRegistry.ts:101`.

```text
Object({
  agentId: AcpRegistryAgentId,
  removed: Boolean,
})
```

### AcpRegistryOperationError

Source: `packages/contracts/src/acpRegistry.ts:330`.

```text
Object({_tag: Const("AcpRegistryOperationError"),
    reason: AcpRegistryOperationErrorReason,
    message: String,
    authMethods: Optional(
      Array(AcpRegistryProbeAuthMethod).check(isMaxLength(32)),
    ),
    authAction: Optional(AcpRegistryUrlAuthAction),
    cause: Optional(Defect()),
  })
```

### AcpRegistryOperationErrorReason

Source: `packages/contracts/src/acpRegistry.ts:302`.

```text
Enum([
  "agent_not_configured",
  "agent_not_found",
  "archive_invalid",
  "authentication_failed",
  "checksum_mismatch",
  "download_failed",
  "install_failed",
  "instance_not_found",
  "logout_unsupported",
  "logout_failed",
  "probe_failed",
  "project_not_found",
  "registry_unavailable",
  "runner_unavailable",
  "session_import_failed",
  "session_delete_unsupported",
  "session_delete_failed",
  "session_list_unsupported",
  "session_resume_unsupported",
  "providers_unsupported",
  "providers_list_failed",
  "provider_configuration_failed",
  "unsupported_distribution",
  "unsupported_platform",
])
```

### AcpRegistryPrepareInput

Source: `packages/contracts/src/acpRegistry.ts:82`.

```text
Object({
  agentId: AcpRegistryAgentId,
})
```

### AcpRegistryPrepareResult

Source: `packages/contracts/src/acpRegistry.ts:87`.

```text
Object({
  agentId: AcpRegistryAgentId,
  version: TrimmedNonEmptyString.check(isMaxLength(128)),
  distribution: AcpRegistryDistribution,
  prepared: Const(true),
})
```

### AcpRegistryProbeAuthMethod

Source: `packages/contracts/src/acpRegistry.ts:111`.

```text
Object({
  id: AcpRegistryProbeText,
  name: AcpRegistryProbeText,
  description: AcpRegistryProbeDescription,
  type: Enum(["agent", "env_var", "terminal"]),
  command: Optional(String.check(isMaxLength(2_048))),
  envVarNames: Optional(Array(AcpRegistryProbeText).check(isMaxLength(16))),
  link: Optional(AcpRegistryUrl),
})
```

### AcpRegistryProbeDescription

Source: `packages/contracts/src/acpRegistry.ts:109`.

```text
Nullable(String.check(isMaxLength(1_024)))
```

### AcpRegistryProbeText

Source: `packages/contracts/src/acpRegistry.ts:108`.

```text
TrimmedNonEmptyString.check(isMaxLength(256))
```

### AcpRegistryProviderId

Source: `packages/contracts/src/acpRegistry.ts:198`.

```text
TrimmedNonEmptyString.check(isMaxLength(256))
```

### AcpRegistryProviderProtocol

Source: `packages/contracts/src/acpRegistry.ts:199`.

```text
TrimmedNonEmptyString.check(isMaxLength(64))
```

### AcpRegistrySearchAgent

Source: `packages/contracts/src/acpRegistry.ts:62`.

```text
Object({
  id: AcpRegistryAgentId,
  name: TrimmedNonEmptyString.check(isMaxLength(160)),
  version: TrimmedNonEmptyString.check(isMaxLength(128)),
  description: String.check(isMaxLength(1_024)),
  authors: Array(String.check(isMaxLength(256))).check(isMaxLength(16)),
  license: Nullable(String.check(isMaxLength(128))),
  website: Nullable(AcpRegistryUrl),
  repository: Nullable(AcpRegistryUrl),
  icon: Nullable(AcpRegistryUrl),
  distribution: AcpRegistryDistribution,
  integrity: AcpRegistryIntegrity,
})
```

### AcpRegistrySearchInput

Source: `packages/contracts/src/acpRegistry.ts:52`.

```text
Object({
  query: TrimmedString.check(isMaxLength(120)),
  limit: Optional(
    Int.check(isBetween({ minimum: 1, maximum: ACP_REGISTRY_MAX_AGENTS }),
  ),
})
```

### AcpRegistrySearchResult

Source: `packages/contracts/src/acpRegistry.ts:77`.

```text
Object({
  agents: Array(AcpRegistrySearchAgent).check(isMaxLength(ACP_REGISTRY_MAX_AGENTS)),
})
```

### AcpRegistrySession

Source: `packages/contracts/src/acpRegistry.ts:145`.

```text
Object({
  sessionId: AcpRegistrySessionId,
  cwd: AcpRegistrySessionPath,
  additionalDirectories: Array(AcpRegistrySessionPath).check(isMaxLength(32)),
  title: Nullable(String.check(isMaxLength(1_024))),
  updatedAt: Nullable(String.check(isMaxLength(128))),
  importedThreadId: Nullable(ThreadId),
})
```

### AcpRegistrySessionId

Source: `packages/contracts/src/acpRegistry.ts:142`.

```text
TrimmedNonEmptyString.check(isMaxLength(1_024))
```

### AcpRegistrySessionPath

Source: `packages/contracts/src/acpRegistry.ts:143`.

```text
TrimmedNonEmptyString.check(isMaxLength(4_096))
```

### AcpRegistrySetProviderInput

Source: `packages/contracts/src/acpRegistry.ts:225`.

```text
Object({
  instanceId: ProviderInstanceId,
  projectId: ProjectId,
  providerId: AcpRegistryProviderId,
  apiType: AcpRegistryProviderProtocol,
  baseUrl: AcpRegistryUrl,
  headers: Optional(
    Record(
      TrimmedNonEmptyString.check(isMaxLength(128)),
      String.check(isMaxLength(8_192)),
    ),
  ),
})
```

### AcpRegistrySetProviderResult

Source: `packages/contracts/src/acpRegistry.ts:241`.

```text
Object({ configured: Const(true) })
```

### AcpRegistryUrl

Source: `packages/contracts/src/acpRegistry.ts:60`.

```text
String.check(isMaxLength(2_048))
```

### AcpRegistryUrlAuthAction

Source: `packages/contracts/src/acpRegistry.ts:122`.

```text
Object({
  elicitationId: AcpRegistryProbeText,
  url: AcpRegistryUrl,
  message: String.check(isMaxLength(1_024)),
  createdAt: Optional(String.check(isMaxLength(128))),
  expiresAt: Optional(String.check(isMaxLength(128))),
})
```

### AgentSessionImportInput

Source: `packages/contracts/src/agentSessions.ts:51`.

```text
Object({
  projectId: ProjectId,
  expectedWorkspaceRoot: Optional(TrimmedNonEmptyString),
})
```

### AgentSessionImportProjectChangedError

Source: `packages/contracts/src/agentSessions.ts:66`.

```text
Object({_tag: Const("AgentSessionImportProjectChangedError"), projectId: ProjectId })
```

### AgentSessionImportProjectNotFoundError

Source: `packages/contracts/src/agentSessions.ts:57`.

```text
Object({_tag: Const("AgentSessionImportProjectNotFoundError"), projectId: ProjectId })
```

### AgentSessionImportResult

Source: `packages/contracts/src/agentSessions.ts:75`.

```text
Object({
  importedCount: NonNegativeInt,
  skippedCount: NonNegativeInt,
})
```

### AgentSessionProjectCandidate

Source: `packages/contracts/src/agentSessions.ts:32`.

```text
Object({
  path: TrimmedNonEmptyString,
  title: TrimmedNonEmptyString,
  projectId: Optional(ProjectId),
  sources: Array(AgentSessionSource),
  threadCount: NonNegativeInt,
  lastActiveAt: Nullable(IsoDateTime),
  alreadyImported: Boolean,
  git: Optional(Nullable(AgentSessionProjectGit)),
})
```

### AgentSessionProjectGit

Source: `packages/contracts/src/agentSessions.ts:26`.

```text
Object({
  remoteKey: Nullable(String),
  repository: Nullable(String),
})
```

### AgentSessionScanError

Source: `packages/contracts/src/agentSessions.ts:81`.

```text
Object({_tag: Const("AgentSessionScanError"),
    operation: Enum(["read-settings", "read-projects"]),
    cause: Defect(),
  })
```

### AgentSessionScanInput

Source: `packages/contracts/src/agentSessions.ts:23`.

```text
Object({})
```

### AgentSessionScanResult

Source: `packages/contracts/src/agentSessions.ts:44`.

```text
Object({
  candidates: Array(AgentSessionProjectCandidate),
  scannedAt: IsoDateTime,
  truncated: Optional(Boolean),
})
```

### AgentSessionSource

Source: `packages/contracts/src/agentSessions.ts:5`.

```text
Enum(["claudeAgent", "codex"])
```

### AssetAccessError

Source: `packages/contracts/src/assets.ts:310`.

```text
Union([
  AssetWorkspaceContextNotFoundError,
  AssetWorkspaceContextResolutionError,
  AssetWorkspaceRootNormalizationError,
  AssetWorkspacePathValidationError,
  AssetPreviewTypeValidationError,
  AssetWorkspaceAssetInspectionError,
  AssetWorkspaceAssetNotFoundError,
  AssetWorkspaceResolutionError,
  AssetAttachmentNotFoundError,
  AssetProjectFaviconResolutionError,
  AssetProjectFaviconInspectionError,
  AssetProjectFaviconNotFoundError,
  AssetSourceControlMediaUrlValidationError,
  AssetSigningKeyLoadError,
])
```

### AssetAttachmentNotFoundError

Source: `packages/contracts/src/assets.ts:242`.

```text
Object({_tag: Const("AssetAttachmentNotFoundError"),
    resource: AssetResource,
  })
```

### AssetCreateUrlInput

Source: `packages/contracts/src/assets.ts:65`.

```text
Object({
  resource: AssetResource,
})
```

### AssetCreateUrlResult

Source: `packages/contracts/src/assets.ts:76`.

```text
Object({
  relativeUrl: TrimmedNonEmptyString.check(isMaxLength(4096)),
  expiresAt: Number,
  sourcePath: Optional(
    TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
  ),
  imageDimensions: Optional(AssetImageDimensions),
  fileRevision: Optional(TrimmedNonEmptyString.check(isMaxLength(256))),
})
```

### AssetImageDimensions

Source: `packages/contracts/src/assets.ts:70`.

```text
Object({
  width: NonNegativeInt.check(isGreaterThanOrEqualTo(1)),
  height: NonNegativeInt.check(isGreaterThanOrEqualTo(1)),
})
```

### AssetPreviewTypeValidationError

Source: `packages/contracts/src/assets.ts:186`.

```text
Object({_tag: Const("AssetPreviewTypeValidationError"),
    resource: AssetResource,
  })
```

### AssetProjectFaviconInspectionError

Source: `packages/contracts/src/assets.ts:265`.

```text
Object({_tag: Const("AssetProjectFaviconInspectionError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetProjectFaviconNotFoundError

Source: `packages/contracts/src/assets.ts:277`.

```text
Object({_tag: Const("AssetProjectFaviconNotFoundError"),
    resource: AssetResource,
  })
```

### AssetProjectFaviconResolutionError

Source: `packages/contracts/src/assets.ts:253`.

```text
Object({_tag: Const("AssetProjectFaviconResolutionError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetResource

Source: `packages/contracts/src/assets.ts:20`.

```text
Union([
  TaggedObject("workspace-file", {
    threadId: ThreadId,
    path: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
  }),
  TaggedObject("media-file", {
    threadId: ThreadId,
    path: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
    disposition: Optional(Const("attachment")),
  }),
  TaggedObject("draft-workspace-file", {
    cwd: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
    path: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
  }),
  TaggedObject("attachment", {
    attachmentId: TrimmedNonEmptyString.check(isMaxLength(256)),
    fileName: Optional(TrimmedNonEmptyString.check(isMaxLength(255))),
    mimeType: Optional(TrimmedNonEmptyString.check(isMaxLength(100))),
    disposition: Optional(Enum(["inline", "attachment"])),
    renderIntent: Optional(Const("mcp-app")),
  }),
  TaggedObject("tool-output-image", {
    threadId: ThreadId,
    itemId: TurnItemId,
    index: NonNegativeInt,
  }),
  TaggedObject("project-favicon", {
    cwd: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
    path: Optional(ProjectFaviconPath),
  }),
  TaggedObject("native-app-icon", {
    app: ToolActivityNativeAppReference,
  }),
  TaggedObject(SOURCE_CONTROL_MEDIA_ASSET, {
    cwd: TrimmedNonEmptyString.check(isMaxLength(ASSET_PATH_MAX_LENGTH)),
    url: TrimmedNonEmptyString.check(isMaxLength(2048)),
  }),
])
```

### AssetSigningKeyLoadError

Source: `packages/contracts/src/assets.ts:288`.

```text
Object({_tag: Const("AssetSigningKeyLoadError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetSourceControlMediaUrlValidationError

Source: `packages/contracts/src/assets.ts:301`.

```text
Object({_tag: Const("AssetGitHubMediaUrlValidationError"),})
```

### AssetWorkspaceAssetInspectionError

Source: `packages/contracts/src/assets.ts:199`.

```text
Object({_tag: Const("AssetWorkspaceAssetInspectionError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetWorkspaceAssetNotFoundError

Source: `packages/contracts/src/assets.ts:213`.

```text
Object({_tag: Const("AssetWorkspaceAssetNotFoundError"),
    resource: AssetResource,
    checkedPath: Optional(String),
  })
```

### AssetWorkspaceContextNotFoundError

Source: `packages/contracts/src/assets.ts:139`.

```text
Object({_tag: Const("AssetWorkspaceContextNotFoundError"),
    resource: AssetResource,
  })
```

### AssetWorkspaceContextResolutionError

Source: `packages/contracts/src/assets.ts:150`.

```text
Object({_tag: Const("AssetWorkspaceContextResolutionError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetWorkspacePathValidationError

Source: `packages/contracts/src/assets.ts:174`.

```text
Object({_tag: Const("AssetWorkspacePathValidationError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetWorkspaceResolutionError

Source: `packages/contracts/src/assets.ts:230`.

```text
Object({_tag: Const("AssetWorkspaceResolutionError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AssetWorkspaceRootNormalizationError

Source: `packages/contracts/src/assets.ts:162`.

```text
Object({_tag: Const("AssetWorkspaceRootNormalizationError"),
    resource: AssetResource,
    cause: Defect(),
  })
```

### AttachmentCreateUploadUrlInput

Source: `packages/contracts/src/assets.ts:110`.

```text
Union([
  ImageAttachmentCreateUploadUrlInput,
  FileAttachmentCreateUploadUrlInput,
])
```

### AttachmentCreateUploadUrlResult

Source: `packages/contracts/src/assets.ts:116`.

```text
Object({
  attachmentId: TrimmedNonEmptyString.check(isMaxLength(256)),
  relativeUrl: TrimmedNonEmptyString.check(isMaxLength(4096)),
  expiresAt: Number,
})
```

### AttachmentDeleteInput

Source: `packages/contracts/src/assets.ts:123`.

```text
Object({
  attachmentId: TrimmedNonEmptyString.check(isMaxLength(256)),
})
```

### AttachmentUploadSigningKeyError

Source: `packages/contracts/src/assets.ts:128`.

```text
Object({_tag: Const("AttachmentUploadSigningKeyError"),
    cause: Defect(),
  })
```

### AuthAccessReadScope

Source: `packages/contracts/src/auth.ts:44`.

```text
"access:read"
```

### AuthAccessSnapshot

Source: `packages/contracts/src/auth.ts:299`.

```text
Object({
  pairingLinks: Array(AuthPairingLink),
  clientSessions: Array(AuthClientSession),
})
```

### AuthAccessStreamClientRemovedEvent

Source: `packages/contracts/src/auth.ts:357`.

```text
Object({
  version: Const(1),
  revision: Number,
  type: Const("clientRemoved"),
  payload: Object({
    sessionId: AuthSessionId,
  }),
})
```

### AuthAccessStreamClientUpsertedEvent

Source: `packages/contracts/src/auth.ts:349`.

```text
Object({
  version: Const(1),
  revision: Number,
  type: Const("clientUpserted"),
  payload: AuthClientSession,
})
```

### AuthAccessStreamError

Source: `packages/contracts/src/auth.ts:333`.

```text
Object({_tag: Const("AuthAccessStreamError"),
    message: String,
  })
```

### AuthAccessStreamEvent

Source: `packages/contracts/src/auth.ts:367`.

```text
Union([
  AuthAccessStreamSnapshotEvent,
  AuthAccessStreamPairingLinkUpsertedEvent,
  AuthAccessStreamPairingLinkRemovedEvent,
  AuthAccessStreamClientUpsertedEvent,
  AuthAccessStreamClientRemovedEvent,
])
```

### AuthAccessStreamPairingLinkRemovedEvent

Source: `packages/contracts/src/auth.ts:322`.

```text
Object({
  version: Const(1),
  revision: Number,
  type: Const("pairingLinkRemoved"),
  payload: Object({
    id: TrimmedNonEmptyString,
  }),
})
```

### AuthAccessStreamPairingLinkUpsertedEvent

Source: `packages/contracts/src/auth.ts:313`.

```text
Object({
  version: Const(1),
  revision: Number,
  type: Const("pairingLinkUpserted"),
  payload: AuthPairingLink,
})
```

### AuthAccessStreamSnapshotEvent

Source: `packages/contracts/src/auth.ts:305`.

```text
Object({
  version: Const(1),
  revision: Number,
  type: Const("snapshot"),
  payload: AuthAccessSnapshot,
})
```

### AuthAccessTokenResult

Source: `packages/contracts/src/auth.ts:231`.

```text
Object({
  access_token: TrimmedNonEmptyString,
  issued_token_type: Const(AuthAccessTokenType),
  token_type: Enum(["Bearer", "DPoP"]),
  expires_in: Number,
  scope: TrimmedNonEmptyString,
})
```

### AuthAccessTokenType

Source: `packages/contracts/src/auth.ts:170`.

```text
"urn:ietf:params:oauth:token-type:access_token"
```

### AuthAccessWriteScope

Source: `packages/contracts/src/auth.ts:45`.

```text
"access:write"
```

### AuthBrowserSessionRequest

Source: `packages/contracts/src/auth.ts:184`.

```text
Object({
  credential: TrimmedNonEmptyString,
})
```

### AuthBrowserSessionResult

Source: `packages/contracts/src/auth.ts:189`.

```text
Object({
  authenticated: Const(true),
  ...authScopeResponseFields,
  sessionMethod: ServerAuthSessionMethod,
  expiresAt: DateTimeUtc,
})
```

### AuthClientMetadata

Source: `packages/contracts/src/auth.ts:275`.

```text
Object({
  label: Optional(TrimmedNonEmptyString),
  ipAddress: Optional(TrimmedNonEmptyString),
  userAgent: Optional(TrimmedNonEmptyString),
  deviceType: AuthClientMetadataDeviceType,
  os: Optional(TrimmedNonEmptyString),
  browser: Optional(TrimmedNonEmptyString),
})
```

### AuthClientMetadataDeviceType

Source: `packages/contracts/src/auth.ts:197`.

```text
Enum([
  "desktop",
  "mobile",
  "tablet",
  "bot",
  "unknown",
])
```

### AuthClientPresentationMetadata

Source: `packages/contracts/src/auth.ts:206`.

```text
Object({
  label: Optional(TrimmedNonEmptyString),
  deviceType: Optional(AuthClientMetadataDeviceType),
  os: Optional(TrimmedNonEmptyString),
  osMajorVersion: Optional(Int),
  deviceModel: Optional(TrimmedNonEmptyString),
  surface: Optional(ClientSurface),
  webDeployment: Optional(ClientWebDeployment),
  browser: Optional(TrimmedNonEmptyString),
  appVersion: Optional(TrimmedNonEmptyString),
})
```

### AuthClientSession

Source: `packages/contracts/src/auth.ts:285`.

```text
Object({
  sessionId: AuthSessionId,
  subject: TrimmedNonEmptyString,
  ...authScopeResponseFields,
  method: ServerAuthSessionMethod,
  client: AuthClientMetadata,
  issuedAt: DateTimeUtc,
  expiresAt: DateTimeUtc,
  lastConnectedAt: Nullable(DateTimeUtc),
  connected: Boolean,
  current: Boolean,
})
```

### AuthClientSessionRevokeResult

Source: `packages/contracts/src/environmentHttp.ts:520`.

```text
Object({
  revoked: Boolean,
})
```

### AuthCreatePairingCredentialInput

Source: `packages/contracts/src/auth.ts:386`.

```text
Object({
  label: Optional(TrimmedNonEmptyString),
  scopes: Optional(AuthGrantScopes),
})
```

### AuthDiagnosticsReadScope

Source: `packages/contracts/src/auth.ts:36`.

```text
"diagnostics:read"
```

### AuthEnvironmentBootstrapTokenType

Source: `packages/contracts/src/auth.ts:171`.

```text
"urn:t3:params:oauth:token-type:environment-bootstrap"
```

### AuthEnvironmentMaintainScope

Source: `packages/contracts/src/auth.ts:34`.

```text
"environment:maintain"
```

### AuthEnvironmentScope

Source: `packages/contracts/src/auth.ts:48`.

```text
Enum([
  AuthOrchestrationReadScope,
  AuthOrchestrationOperateScope,
  AuthSettingsWriteScope,
  AuthProvidersManageScope,
  AuthEnvironmentMaintainScope,
  AuthPreviewOperateScope,
  AuthDiagnosticsReadScope,
  AuthTerminalReadScope,
  AuthTerminalOperateScope,
  AuthFilesystemReadScope,
  AuthFilesystemWriteScope,
  AuthReviewWriteScope,
  AuthSourceControlWriteScope,
  AuthAccessReadScope,
  AuthAccessWriteScope,
  AuthRelayReadScope,
  AuthRelayWriteScope,
])
```

### AuthEnvironmentScopes

Source: `packages/contracts/src/auth.ts:68`.

```text
Array(AuthEnvironmentScope)
```

### AuthFilesystemReadScope

Source: `packages/contracts/src/auth.ts:40`.

```text
"filesystem:read"
```

### AuthFilesystemWriteScope

Source: `packages/contracts/src/auth.ts:41`.

```text
"filesystem:write"
```

### AuthGrantScope

Source: `packages/contracts/src/auth.ts:71`.

```text
Enum(all AuthEnvironmentScope values except "review:write")
```

### AuthGrantScopes

Source: `packages/contracts/src/auth.ts:75`.

```text
Array(AuthGrantScope)
```

### AuthMcpApprovalDecision

Source: `packages/contracts/src/auth.ts:494`.

```text
Union([
  TaggedObject("deny", {}),
  TaggedObject("pairing-code", {
    access: AuthMcpClientAccess,
    code: TrimmedNonEmptyString,
  }),
  TaggedObject("browser-session", {
    access: AuthMcpClientAccess,
    csrfToken: String,
  }),
])
```

### AuthMcpApprovalDecisionRequest

Source: `packages/contracts/src/auth.ts:507`.

```text
Object({
  authorization: AuthMcpAuthorizationRequest,
  decision: AuthMcpApprovalDecision,
})
```

### AuthMcpApprovalDetails

Source: `packages/contracts/src/auth.ts:478`.

```text
Object({
  clientName: String,
  redirectHost: String,
  environmentHost: String,
  csrfToken: Optional(String),
  oneClickAccess: Optional(Array(AuthMcpClientAccess)),
})
```

### AuthMcpApprovalError

Source: `packages/contracts/src/auth.ts:513`.

```text
Object({_tag: Const("AuthMcpApprovalError"), message: String })
```

### AuthMcpApprovalRedirect

Source: `packages/contracts/src/auth.ts:489`.

```text
Object({
  redirectTo: String,
})
```

### AuthMcpAuthorizationRequest

Source: `packages/contracts/src/auth.ts:466`.

```text
Object({
  response_type: Optional(String),
  client_id: Optional(String),
  redirect_uri: Optional(String),
  code_challenge: Optional(String),
  code_challenge_method: Optional(String),
  state: Optional(String),
  resource: Optional(String),
})
```

### AuthMcpAuthorizationServerMetadata

Source: `packages/contracts/src/auth.ts:420`.

```text
Object({
  issuer: String,
  authorization_endpoint: String,
  token_endpoint: String,
  registration_endpoint: String,
  response_types_supported: Array(Const("code")),
  grant_types_supported: Array(Const("authorization_code")),
  code_challenge_methods_supported: Array(Const("S256")),
  token_endpoint_auth_methods_supported: Array(Const("none")),
  scopes_supported: Array(AuthEnvironmentScope),
  authorization_response_iss_parameter_supported: Boolean,
})
```

### AuthMcpClientAccess

Source: `packages/contracts/src/auth.ts:402`.

```text
Enum([
  "read-only",
  "approval-required",
  "auto-accept-edits",
  "auto",
  "full-access",
])
```

### AuthMcpClientRegistration

Source: `packages/contracts/src/auth.ts:434`.

```text
Object({
  client_name: Optional(String),
  redirect_uris: Optional(Array(String)),
  token_endpoint_auth_method: Optional(String),
})
```

### AuthMcpProtectedResourceMetadata

Source: `packages/contracts/src/auth.ts:411`.

```text
Object({
  resource: String,
  authorization_servers: Array(String),
  scopes_supported: Array(AuthEnvironmentScope),
  bearer_methods_supported: Array(Const("header")),
  resource_name: String,
})
```

### AuthMcpRegisteredClient

Source: `packages/contracts/src/auth.ts:441`.

```text
Object({
  client_id: String,
  client_name: String,
  redirect_uris: Array(String),
  grant_types: Array(Const("authorization_code")),
  response_types: Array(Const("code")),
  token_endpoint_auth_method: Const("none"),
})
```

### AuthMcpTokenRequest

Source: `packages/contracts/src/auth.ts:519`.

```text
Object({
  grant_type: Optional(String),
  code: Optional(String),
  redirect_uri: Optional(String),
  client_id: Optional(String),
  code_verifier: Optional(String),
  resource: Optional(String),
})
```

### AuthMcpTokenResult

Source: `packages/contracts/src/auth.ts:529`.

```text
Object({
  access_token: String,
  token_type: Const("Bearer"),
  expires_in: Number,
  scope: String,
})
```

### AuthOrchestrationOperateScope

Source: `packages/contracts/src/auth.ts:31`.

```text
"orchestration:operate"
```

### AuthOrchestrationReadScope

Source: `packages/contracts/src/auth.ts:30`.

```text
"orchestration:read"
```

### AuthOtherClientSessionsRevokeResult

Source: `packages/contracts/src/environmentHttp.ts:525`.

```text
Object({
  revokedCount: Number,
})
```

### AuthPairingCredentialResult

Source: `packages/contracts/src/auth.ts:257`.

```text
Object({
  id: TrimmedNonEmptyString,
  credential: TrimmedNonEmptyString,
  label: Optional(TrimmedNonEmptyString),
  expiresAt: DateTimeUtc,
})
```

### AuthPairingLink

Source: `packages/contracts/src/auth.ts:265`.

```text
Object({
  id: TrimmedNonEmptyString,
  ...authScopeResponseFields,
  subject: TrimmedNonEmptyString,
  label: Optional(TrimmedNonEmptyString),
  createdAt: DateTimeUtc,
  expiresAt: DateTimeUtc,
})
```

### AuthPairingLinkRevokeResult

Source: `packages/contracts/src/environmentHttp.ts:515`.

```text
Object({
  revoked: Boolean,
})
```

### AuthPreviewOperateScope

Source: `packages/contracts/src/auth.ts:35`.

```text
"preview:operate"
```

### AuthProvidersManageScope

Source: `packages/contracts/src/auth.ts:33`.

```text
"providers:manage"
```

### AuthRelayReadScope

Source: `packages/contracts/src/auth.ts:46`.

```text
"relay:read"
```

### AuthRelayWriteScope

Source: `packages/contracts/src/auth.ts:47`.

```text
"relay:write"
```

### AuthReviewWriteScope

Source: `packages/contracts/src/auth.ts:43`.

```text
"review:write"
```

### AuthRevokeClientSessionInput

Source: `packages/contracts/src/auth.ts:381`.

```text
Object({
  sessionId: AuthSessionId,
})
```

### AuthRevokePairingLinkInput

Source: `packages/contracts/src/auth.ts:376`.

```text
Object({
  id: TrimmedNonEmptyString,
})
```

### AuthSessionState

Source: `packages/contracts/src/auth.ts:392`.

```text
Object({
  authenticated: Boolean,
  auth: ServerAuthDescriptor,
  scopes: Optional(AuthEnvironmentScopes),
  permissions: authScopeResponseFields.permissions,
  sessionMethod: Optional(ServerAuthSessionMethod),
  expiresAt: Optional(DateTimeUtc),
})
```

### AuthSettingsWriteScope

Source: `packages/contracts/src/auth.ts:32`.

```text
"settings:write"
```

### AuthSourceControlWriteScope

Source: `packages/contracts/src/auth.ts:39`.

```text
"source-control:write"
```

### AuthStandardClientScopes

Source: `packages/contracts/src/auth.ts:149`.

```text
[
  AuthOrchestrationReadScope,
  AuthOrchestrationOperateScope,
  AuthSettingsWriteScope,
  AuthProvidersManageScope,
  AuthEnvironmentMaintainScope,
  AuthPreviewOperateScope,
  AuthDiagnosticsReadScope,
  AuthTerminalReadScope,
  AuthTerminalOperateScope,
  AuthSourceControlWriteScope,
  AuthFilesystemReadScope,
  AuthFilesystemWriteScope,
  AuthRelayReadScope,
  AuthAccessReadScope,
  AuthAccessWriteScope,
  AuthRelayWriteScope,
]
```

### AuthStatusPayload

Source: `packages/contracts/src/providerRuntime.ts:647`.

```text
Object({
  isAuthenticating: Optional(Boolean),
  output: Optional(Array(String)),
  error: Optional(TrimmedNonEmptyStringSchema),
})
```

### AuthStatusType

Source: `packages/contracts/src/providerRuntime.ts:189`.

```text
Const("auth.status")
```

### AuthTerminalOperateScope

Source: `packages/contracts/src/auth.ts:38`.

```text
"terminal:operate"
```

### AuthTerminalReadScope

Source: `packages/contracts/src/auth.ts:37`.

```text
"terminal:read"
```

### AuthTokenExchangeGrantType

Source: `packages/contracts/src/auth.ts:168`.

```text
"urn:ietf:params:oauth:grant-type:token-exchange"
```

### AuthTokenExchangeRequest

Source: `packages/contracts/src/auth.ts:219`.

```text
Object({
  grant_type: Const(AuthTokenExchangeGrantType),
  subject_token: TrimmedNonEmptyString,
  subject_token_type: Const(AuthEnvironmentBootstrapTokenType),
  requested_token_type: Const(AuthAccessTokenType),
  scope: Optional(TrimmedNonEmptyString),
  client_label: Optional(TrimmedNonEmptyString),
  client_device_type: Optional(AuthClientMetadataDeviceType),
  client_os: Optional(TrimmedNonEmptyString),
})
```

### AuthWebSocketTicketParams

Source: `packages/contracts/src/auth.ts:255`.

```text
Object({ use: AuthWebSocketTicketUse })
```

### AuthWebSocketTicketResult

Source: `packages/contracts/src/auth.ts:240`.

```text
Object({
  ticket: TrimmedNonEmptyString,
  expiresAt: DateTimeUtc,
})
```

### AuthWebSocketTicketUse

Source: `packages/contracts/src/auth.ts:252`.

```text
Enum(["rpc", "media"])
```

### BROWSER_PROFILE_MAX_COUNT

Source: `packages/contracts/src/browserProfile.ts:5`.

```text
24
```

### BROWSER_PROFILE_NAME_MAX_LENGTH

Source: `packages/contracts/src/browserProfile.ts:4`.

```text
48
```

### BROWSER_RECORDING_FRAME_RATES

Source: `packages/contracts/src/preview.ts:227`.

```text
[30, 60]
```

### BackgroundActivityOverrides

Source: `packages/contracts/src/settings.ts:893`.

```text
Object({
  automaticGitFetchInterval: Optional(DurationFromMillis),
  providerHealthRefreshInterval: Optional(DurationFromMillis),
  hostPowerMonitorActiveInterval: Optional(DurationFromMillis),
  hostPowerMonitorIdleInterval: Optional(DurationFromMillis),
  idleClientTtl: Optional(DurationFromMillis),
  pauseWhenHostLocked: Optional(Boolean),
  pauseWhenHostLowPower: Optional(Boolean),
  pauseWhenClientLowPower: Optional(Boolean),
  pauseWhenOnBattery: Optional(Boolean),
})
```

### BackgroundActivityProfile

Source: `packages/contracts/src/settings.ts:877`.

```text
Enum([
  "balanced",
  "performance",
  "battery-saver",
])
```

### BackgroundActivityProfileSelection

Source: `packages/contracts/src/settings.ts:885`.

```text
Enum([
  "balanced",
  "performance",
  "battery-saver",
  "custom",
])
```

### BackgroundActivitySettings

Source: `packages/contracts/src/settings.ts:906`.

```text
Object({
  schemaVersion: Const(1),
  profile: BackgroundActivityProfileSelection,
  baseProfile: Optional(BackgroundActivityProfile),
  overrides: BackgroundActivityOverrides,
})
```

### BackgroundBooleanState

Source: `packages/contracts/src/background.ts:12`.

```text
Enum(["true", "false", "unknown"])
```

### BackgroundPolicySnapshot

Source: `packages/contracts/src/background.ts:102`.

```text
Object({
  hostPower: HostPowerSnapshot,
  leases: Array(ClientActivityLease),
  activeForegroundLeaseCount: Number,
  activeScopeKeys: Array(String),
  shouldRunOpportunisticWork: Boolean,
  updatedAt: DateTimeUtc,
})
```

### BackgroundScope

Source: `packages/contracts/src/background.ts:48`.

```text
Union([
  Object({ type: Const("server-config") }),
  Object({
    type: Const("provider-status"),
    instanceId: Optional(ProviderInstanceId),
  }),
  Object({ type: Const("vcs-status"), cwd: String }),
  Object({ type: Const("git-refs"), cwd: String }),
  Object({ type: Const("diagnostics") }),
  Object({ type: Const("thread"), threadId: ThreadId }),
])
```

### BooleanProviderOptionDescriptor

Source: `packages/contracts/src/model.ts:33`.

```text
Object({
  ...ProviderOptionDescriptorBase,
  type: Const("boolean"),
  currentValue: Optional(Boolean),
})
```

### BoundedString

Source: `packages/contracts/src/composerContext.ts:66`.

```text
String with maximum length max.
```

### BranchNamingMode

Source: `packages/contracts/src/settings.ts:865`.

```text
Enum(["static", "semantic", "custom"])
```

### BrowserProfile

Source: `packages/contracts/src/browserProfile.ts:21`.

```text
Object({
  id: BrowserProfileId,
  name: BrowserProfileName,
  kind: BrowserProfileKind,
})
```

### BrowserProfileId

Source: `packages/contracts/src/browserProfile.ts:8`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(64),
  isPattern(/^[^\p{Cc}]+$/u),
)
```

### BrowserProfileKind

Source: `packages/contracts/src/browserProfile.ts:18`.

```text
Enum(["persistent", "incognito"])
```

### BrowserProfileName

Source: `packages/contracts/src/browserProfile.ts:14`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(BROWSER_PROFILE_NAME_MAX_LENGTH),
)
```

### BrowserRecordingFrameRate

Source: `packages/contracts/src/preview.ts:228`.

```text
Enum(BROWSER_RECORDING_FRAME_RATES)
```

### ByteCount

Source: `packages/contracts/src/providerSetup.ts:155`.

```text
Int.check(isGreaterThanOrEqualTo(0))
```

### CHAT_ATTACHMENT_ID_MAX_CHARS

Source: `packages/contracts/src/chatAttachment.ts:32`.

```text
128
```

### COMPOSER_CONTEXT_ELEMENT_HTML_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:37`.

```text
8_000
```

### COMPOSER_CONTEXT_ELEMENT_STYLES_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:38`.

```text
8_000
```

### COMPOSER_CONTEXT_LABEL_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:35`.

```text
200
```

### COMPOSER_CONTEXT_MAX_RECORDS

Source: `packages/contracts/src/composerContext.ts:253`.

```text
200
```

### COMPOSER_CONTEXT_MAX_SERIALIZED_CHARS

Source: `packages/contracts/src/composerContext.ts:254`.

```text
16_000_000
```

### COMPOSER_CONTEXT_PREVIEW_COMMENT_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:41`.

```text
8_000
```

### COMPOSER_CONTEXT_REVIEW_DIFF_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:40`.

```text
32_000
```

### COMPOSER_CONTEXT_REVIEW_TEXT_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:39`.

```text
16_000
```

### COMPOSER_CONTEXT_TERMINAL_TEXT_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:36`.

```text
64_000
```

### CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS

Source: `packages/contracts/src/preview.ts:6`.

```text
32
```

### CONTEXT_ID_MAX_CHARS

Source: `packages/contracts/src/composerContext.ts:30`.

```text
128
```

### ChatAttachment

Source: `packages/contracts/src/chatAttachment.ts:201`.

```text
Union([
  ChatImageAttachment,
  ChatFileAttachment,
  ChatUnknownAttachment,
])
```

### ChatAttachmentId

Source: `packages/contracts/src/chatAttachment.ts:34`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(CHAT_ATTACHMENT_ID_MAX_CHARS),
  isPattern(/^[a-z0-9_-]+$/i),
)
```

### ChatFileAttachment

Source: `packages/contracts/src/chatAttachment.ts:161`.

```text
Object({
  type: Const("file"),
  id: ChatAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt.check(
    isGreaterThanOrEqualTo(1),
    isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_FILE_BYTES),
  ),
  source: Optional(PastedTextAttachmentSource),
})
```

### ChatGptHandoffInput

Source: `packages/contracts/src/providerSetup.ts:239`.

```text
Object({
  instanceId: ProviderInstanceId,
  environmentId: EnvironmentId,
  attemptId: String.check(isMaxLength(128)),
  returnUrl: String.check(isMaxLength(4_096)),
  profile: Nullable(ChatGptReconnectProfile),
})
```

### ChatGptHandoffState

Source: `packages/contracts/src/providerSetup.ts:247`.

```text
Union([
  Object({ phase: Const("auth"), state: ProviderAuthState }),
  Object({ phase: Const("finished"), profile: ChatGptTransferredProfile }),
])
```

### ChatGptImportProfileInput

Source: `packages/contracts/src/providerSetup.ts:235`.

```text
Object({
  instanceId: ProviderInstanceId,
  profile: ChatGptTransferredProfile,
})
```

### ChatGptReconnectProfile

Source: `packages/contracts/src/providerSetup.ts:201`.

```text
Object({
  clientId: String.check(isPattern(/^oaiapp_[\w-]+$/u)),
  subject: Optional(String),
  email: Optional(Nullable(String)),
  redirectUri: Optional(
    String.check(
      isPattern(/^http:\/\/(?:127\.0\.0\.1|localhost):[1-9]\d{0,4}\/auth\/callback$/u),
    ),
  ),
  connectionLabel: Optional(String),
  sharingEnabled: Optional(Boolean),
  idTokenHint: Optional(String.check(isMaxLength(16_384))),
})
```

### ChatGptReconnectProfileInput

Source: `packages/contracts/src/providerSetup.ts:231`.

```text
Object({
  instanceId: ProviderInstanceId,
  methodId: String,
})
```

### ChatGptTransferredProfile

Source: `packages/contracts/src/providerSetup.ts:215`.

```text
Object({
  registration: ChatGptReconnectProfile,
  credentials: Object({
    clientId: String,
    accessToken: NonEmptyString.check(isMaxLength(16_384)),
    refreshToken: Nullable(String.check(isMaxLength(16_384))),
    idToken: NonEmptyString.check(isMaxLength(16_384)),
    issuer: String,
    expiresAt: Finite,
    earliestRefreshAt: Nullable(Finite),
    scopes: Array(String),
    subject: String,
    email: Nullable(String),
  }),
})
```

### ChatImageAttachment

Source: `packages/contracts/src/chatAttachment.ts:148`.

```text
Object({
  type: Const("image"),
  id: ChatAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100), isPattern(/^image\
  sizeBytes: NonNegativeInt.check(isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES)),
  source: Optional(SnapShotSource),
});
export type ChatImageAttachment = typeof ChatImageAttachment.Type;
export const PastedTextAttachmentSource = TaggedObject("pasted-text", {});
export type PastedTextAttachmentSource = typeof PastedTextAttachmentSource.Type;
export const ChatFileAttachment = Object({
  type: Const("file"),
  id: ChatAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt.check(
    isGreaterThanOrEqualTo(1),
    isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_FILE_BYTES),
  ),
  source: Optional(PastedTextAttachmentSource),
});
export type ChatFileAttachment = typeof ChatFileAttachment.Type;
export const ChatUnknownAttachment = Object({
  type: TrimmedNonEmptyString.check(
    isMaxLength(50),
    isPattern(/^(?!(?:image|file)$)/),
  ),
  id: ChatAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt,
});
export type ChatUnknownAttachment = typeof ChatUnknownAttachment.Type;
export const UploadChatImageAttachment = Object({
  id: Optional(ChatAttachmentId),
  type: Const("image"),
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100), isPattern(/^image\
  sizeBytes: NonNegativeInt.check(isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES)),
  dataUrl: TrimmedNonEmptyString.check(
    isMaxLength(PROVIDER_SEND_TURN_MAX_IMAGE_DATA_URL_CHARS),
  ),
  source: Optional(SnapShotSource),
});
export type UploadChatImageAttachment = typeof UploadChatImageAttachment.Type;
export const ChatAttachment = Union([
  ChatImageAttachment,
  ChatFileAttachment,
  ChatUnknownAttachment,
]);
export type ChatAttachment = typeof ChatAttachment.Type;
export function getProviderAttachmentLimitError(
  attachments: ReadonlyArray<Pick<ChatAttachment, "type" | "mimeType" | "sizeBytes">>,
): string | undefined {
  if (attachments.length > PROVIDER_SEND_TURN_MAX_ATTACHMENTS) {
    return `You can attach up to ${PROVIDER_SEND_TURN_MAX_ATTACHMENTS} files per message or question response.`;
  }
  const imageBytes = attachments.reduce(
    (total, attachment) =>
      total +
      (attachment.type === "image" || isProviderSendTurnSupportedImageMimeType(attachment.mimeType)
        ? attachment.sizeBytes
        : 0),
    0,
  );
  if (imageBytes > PROVIDER_SEND_TURN_MAX_TOTAL_IMAGE_BYTES) {
    return "Images can total up to 80 MiB per message or question response. Use smaller images or send fewer at once.";
  }
}
export const UploadChatAttachment = Union([UploadChatImageAttachment]);
export type UploadChatAttachment = typeof UploadChatAttachment.Type;
export const PersistChatAttachmentsInput = Object({
  threadId: ThreadId,
  messageId: MessageId,
  attachments: Array(UploadChatAttachment),
});
export type PersistChatAttachmentsInput = typeof PersistChatAttachmentsInput.Type;
export const PersistChatAttachmentsResult = Object({
  attachments: Array(ChatAttachment),
});
export type PersistChatAttachmentsResult = typeof PersistChatAttachmentsResult.Type;
export class PersistChatAttachmentsError extends TaggedError<PersistChatAttachmentsError>()(
  "PersistChatAttachmentsError",
  {
    message: String,
    cause: Optional(Defect()),
  },
) {}
```

### ChatUnknownAttachment

Source: `packages/contracts/src/chatAttachment.ts:176`.

```text
Object({
  type: TrimmedNonEmptyString.check(
    isMaxLength(50),
    isPattern(/^(?!(?:image|file)$)/),
  ),
  id: ChatAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt,
})
```

### ClientActivityClientId

Source: `packages/contracts/src/background.ts:64`.

```text
TrimmedNonEmptyString.check(isMaxLength(128))
```

### ClientActivityLease

Source: `packages/contracts/src/background.ts:84`.

```text
Object({
  sessionId: AuthSessionId,
  rpcClientId: RpcClientId,
  clientId: ClientActivityClientId,
  clientKind: ClientKind,
  visible: Boolean,
  focused: Boolean,
  recentlyInteracted: Boolean,
  appState: Optional(Enum(["active", "inactive", "background", "unknown"])),
  lowPowerMode: Optional(BackgroundBooleanState),
  batteryState: Optional(Enum(["unknown", "unplugged", "charging", "full"])),
  networkType: Optional(String),
  scopes: Array(BackgroundScope),
  updatedAt: DateTimeUtc,
  expiresAt: DateTimeUtc,
})
```

### ClientActivityReportInput

Source: `packages/contracts/src/background.ts:67`.

```text
Object({
  environmentId: Optional(EnvironmentId),
  clientId: ClientActivityClientId,
  clientKind: ClientKind,
  visible: Boolean,
  focused: Boolean,
  recentlyInteracted: Boolean,
  appState: Optional(Enum(["active", "inactive", "background", "unknown"])),
  lowPowerMode: Optional(BackgroundBooleanState),
  batteryState: Optional(Enum(["unknown", "unplugged", "charging", "full"])),
  networkType: Optional(String),
  scopes: Array(BackgroundScope),
  ttlMs: Optional(Number),
  observedAt: DateTimeUtc,
})
```

### ClientKind

Source: `packages/contracts/src/background.ts:61`.

```text
Enum(["web", "desktop-renderer", "mobile", "unknown"])
```

### ClientSurface

Source: `packages/contracts/src/baseSchemas.ts:279`.

```text
Enum(["web", "desktop", "mobile", "cli"])
```

### ClientWebDeployment

Source: `packages/contracts/src/baseSchemas.ts:297`.

```text
Enum(["hosted", "server"])
```

### CodexAuthCallbackInput

Source: `packages/contracts/src/providerSetup.ts:17`.

```text
Object({
  authorizationUrl: String.check(isMaxLength(16_384)),
  returnUrl: String.check(isMaxLength(4_096)),
  environmentId: EnvironmentId,
  instanceId: ProviderInstanceId,
  flowId: TrimmedNonEmptyString.check(isMaxLength(128)),
})
```

### CodexAuthCallbackState

Source: `packages/contracts/src/providerSetup.ts:25`.

```text
Union([
  Object({ phase: Const("ready") }),
  Object({
    phase: Const("finished"),
    callbackUrl: String.check(isMaxLength(16_384)),
  }),
])
```

### CommandNotificationSource

Source: `packages/contracts/src/orchestrationV2.ts:1142`.

```text
Object({ kind: Const("command") })
```

### CommentBody

Source: `packages/contracts/src/pullRequest.ts:737`.

```text
String.check(isNonEmpty()).check(isMaxLength(65_536))
```

### ComposerContextId

Source: `packages/contracts/src/composerContext.ts:43`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(CONTEXT_ID_MAX_CHARS),
  isPattern(CONTEXT_ID_PATTERN),
)
```

### ComposerContextKind

Source: `packages/contracts/src/composerContext.ts:55`.

```text
TrimmedNonEmptyString.check(
  isPattern(CONTEXT_KIND_PATTERN),
)
```

### ComposerContextRecord

Source: `packages/contracts/src/composerContext.ts:247`.

```text
Union([
  ...KnownComposerContextRecord.members,
  UnknownContextRecord,
])
```

### ConfiguredDeviceHostId

Source: `packages/contracts/src/device.ts:16`.

```text
DeviceHostId.check(
  isPattern(/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/),
  Predicate('The local host id is reserved.'),
)
```

### ConfiguredLocalServerUrls

Source: `packages/contracts/src/preview.ts:10`.

```text
Array(Url).check(
  isMaxLength(CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS),
)
```

### ContextAttachmentId

Source: `packages/contracts/src/composerContext.ts:61`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(CONTEXT_ID_MAX_CHARS),
  isPattern(CONTEXT_ID_PATTERN),
)
```

### ContextLabel

Source: `packages/contracts/src/composerContext.ts:65`.

```text
String.check(isMaxLength(COMPOSER_CONTEXT_LABEL_MAX_CHARS))
```

### DEFAULT_BACKGROUND_ACTIVITY_PROFILE

Source: `packages/contracts/src/settings.ts:883`.

```text
"balanced"
```

### DEFAULT_RUNTIME_MODE

Source: `packages/contracts/src/providerPolicy.ts:32`.

```text
"full-access"
```

### DEFAULT_SIDEBAR_AUTO_SETTLE_AFTER_DAYS

Source: `packages/contracts/src/settings.ts:94`.

```text
3
```

### DEFAULT_TEXT_GENERATION_MODEL

Source: `packages/contracts/src/model.ts:132`.

```text
"gpt-6-luna"
```

### DEFAULT_TEXT_GENERATION_REASONING_EFFORT

Source: `packages/contracts/src/model.ts:135`.

```text
"low"
```

### DatabaseStorageStatus

Source: `packages/contracts/src/settings.ts:1044`.

```text
Object({
  databaseBytes: Number,
  reclaimableBytes: Number,
  incrementalVacuumEnabled: Boolean,
  compactCommand: String,
})
```

### DedupedBodyMarker

Source: `packages/contracts/src/orchestrationV2.ts:789`.

```text
Optional(Const(true))
```

### DefaultThemePreference

Source: `packages/contracts/src/settings.ts:248`.

```text
String.check(isMaxLength(64))
```

### DesktopElectronProcessType

Source: `packages/contracts/src/resourceTelemetry.ts:230`.

```text
Enum([
  "Browser",
  "Tab",
  "Utility",
  "Zygote",
  "Sandbox helper",
  "GPU",
  "Pepper Plugin",
  "Pepper Plugin Broker",
  "Unknown",
])
```

### DesktopUpdateCommitInput

Source: `packages/contracts/src/server.ts:869`.

```text
Object({
  requestId: TrimmedNonEmptyString,
})
```

### DeviceActionInput

Source: `packages/contracts/src/device.ts:372`.

```text
Union([
  Object({
    ...DeviceTarget,
    type: Const("setAppearance"),
    value: DeviceAppearance,
  }),
  Object({ ...DeviceTarget, type: Const("setTextSize"), value: DeviceTextSize }),
  Object({
    ...DeviceTarget,
    type: Const("setToggle"),
    setting: Enum([
      "reduceMotion",
      "increaseContrast",
      "reduceTransparency",
      "showBorders",
      "voiceOver",
      "networkEnabled",
    ]),
    value: Boolean,
  }),
  Object({
    ...DeviceTarget,
    type: Const("setLiquidGlass"),
    value: Enum(["clear", "tinted"]),
  }),
  Object({
    ...DeviceTarget,
    type: Const("setColorFilter"),
    value: DeviceColorFilter,
  }),
  Object({
    ...DeviceTarget,
    type: Const("setOrientation"),
    value: DeviceOrientation,
  }),
  Object({
    ...DeviceTarget,
    type: Const("setLocation"),
    latitude: Number.check(isBetween({ minimum: -90, maximum: 90 }),
    longitude: Number.check(isBetween({ minimum: -180, maximum: 180 }),
  }),
  Object({ ...DeviceTarget, type: Const("clearLocation") }),
  Object({
    ...DeviceTarget,
    type: Const("setPermission"),
    appId: TrimmedNonEmptyString,
    permission: DevicePermission,
    decision: Enum(["grant", "revoke", "reset"]),
  }),
  Object({ ...DeviceTarget, type: Const("openUrl"), url: TrimmedNonEmptyString }),
  Object({
    ...DeviceTarget,
    type: Const("launchApp"),
    appId: TrimmedNonEmptyString,
  }),
  Object({
    ...DeviceTarget,
    type: Const("terminateApp"),
    appId: TrimmedNonEmptyString,
  }),
  Object({ ...DeviceTarget, type: Const("shake") }),
  Object({
    ...DeviceTarget,
    type: Const("sendPush"),
    appId: TrimmedNonEmptyString,
    payload: Union([String, Record(String, Unknown)]),
  }),
])
```

### DeviceActionUnavailableError

Source: `packages/contracts/src/device.ts:532`.

```text
Object({_tag: Const("DeviceActionUnavailableError"),
    operation: String,
    platform: DevicePlatform,
    reason: Enum(["unsupported", "helper_missing"]),
  })
```

### DeviceAppearance

Source: `packages/contracts/src/device.ts:296`.

```text
Enum(["light", "dark"])
```

### DeviceBootError

Source: `packages/contracts/src/device.ts:486`.

```text
Object({_tag: Const("DeviceBootError"),
  hostId: DeviceHostId,
  deviceId: DeviceId,
  reason: Enum(["disk_space", "timeout", "launch_failed"]),
  diagnostics: Optional(String.check(isMaxLength(2_000))),
  cause: Defect(),
})
```

### DeviceCloseInput

Source: `packages/contracts/src/device.ts:279`.

```text
Object({
  hostId: Optional(DeviceHostId),
  threadId: ThreadId,
  deviceId: Optional(DeviceId),
  shutdown: Optional(Boolean),
})
```

### DeviceColorFilter

Source: `packages/contracts/src/device.ts:302`.

```text
Enum([
  "none",
  "grayscale",
  "red-green",
  "green-red",
  "blue-yellow",
])
```

### DeviceConfigureInput

Source: `packages/contracts/src/device.ts:262`.

```text
Object({
  enabled: Optional(Boolean),
  agentAccessEnabled: Optional(Boolean),
  onboardingCompleted: Optional(Boolean),
})
```

### DeviceDetail

Source: `packages/contracts/src/device.ts:343`.

```text
Object({
  hostId: DeviceHostId,
  deviceId: DeviceId,
  settings: DeviceSettings,
  foregroundApp: Nullable(DeviceForegroundApp),
  readAt: String,
})
```

### DeviceDetailInput

Source: `packages/contracts/src/device.ts:443`.

```text
Object(DeviceTarget)
```

### DeviceEasAccount

Source: `packages/contracts/src/device.ts:95`.

```text
Object({
  signedInAs: String,
  credentialSource: Enum(["settings", "env", "eas-login"]),
  apps: Array(Object({ id: String, fullName: String }),
})
```

### DeviceEasAccountError

Source: `packages/contracts/src/device.ts:103`.

```text
Object({_tag: Const("DeviceHostUnavailableError"),
    hostId: DeviceHostId,
    hostLabel: Optional(String),
    reason: String,
    cause: Optional(Defect()),
  })
```

### DeviceEasAccountInput

Source: `packages/contracts/src/device.ts:88`.

```text
Object({
  hostId: Optional(DeviceHostId),
  accessToken: Optional(TrimmedString),
})
```

### DeviceError

Source: `packages/contracts/src/device.ts:547`.

```text
Union([
  DeviceHostUnavailableError,
  DevicePlatformUnavailableError,
  DeviceNotFoundError,
  DeviceBootError,
  DeviceOperationError,
  DeviceActionUnavailableError,
])
```

### DeviceForegroundApp

Source: `packages/contracts/src/device.ts:336`.

```text
Object({
  id: String,
  name: Optional(String),
  version: Optional(String),
})
```

### DeviceHostId

Source: `packages/contracts/src/device.ts:10`.

```text
TrimmedNonEmptyString.check(isMaxLength(128))
```

### DeviceHostStatus

Source: `packages/contracts/src/device.ts:167`.

```text
Enum([
  "disabled",
  "idle",
  "installing",
  "starting",
  "ready",
  "failed",
])
```

### DeviceHostStatuses

Source: `packages/contracts/src/device.ts:186`.

```text
Record(
  DeviceHostId,
  Object({
    status: DeviceHostStatus,
    detail: Optional(String),
    androidDiscoveryIncomplete: Optional(Boolean),
  })
```

### DeviceHostSummary

Source: `packages/contracts/src/device.ts:164`.

```text
Union([MachineDeviceHostSummary, EasDeviceHostSummary])
```

### DeviceHostUnavailableError

Source: `packages/contracts/src/device.ts:446`.

```text
Object({_tag: Const("DeviceHostUnavailableError"),
    hostId: DeviceHostId,
    hostLabel: Optional(String),
    reason: String,
    cause: Optional(Defect()),
  })
```

### DeviceId

Source: `packages/contracts/src/device.ts:113`.

```text
TrimmedNonEmptyString.check(isMaxLength(256))
```

### DeviceListInput

Source: `packages/contracts/src/device.ts:255`.

```text
Object({
  updateTool: Optional(Enum(["hub", "agent"])),
  inspectOnly: Optional(Boolean),
  retryHostId: Optional(DeviceHostId),
})
```

### DeviceNotFoundError

Source: `packages/contracts/src/device.ts:474`.

```text
Object({_tag: Const("DeviceNotFoundError"),
    hostId: DeviceHostId,
    deviceId: DeviceId,
  })
```

### DeviceOpenInput

Source: `packages/contracts/src/device.ts:269`.

```text
Object({
  threadId: ThreadId,
  hostId: Optional(DeviceHostId),
  deviceId: DeviceId,
  platform: DevicePlatform,
  boot: Optional(Boolean),
})
```

### DeviceOperationError

Source: `packages/contracts/src/device.ts:505`.

```text
Object({_tag: Const("DeviceOperationError"),
    operation: String,
    reason: Enum([
      "command_failed",
      "request_failed",
      "invalid_payload",
      "settings_failed",
      "hub_rejected",
    ]),
    exitCode: Optional(Number),
    cause: Defect(),
  })
```

### DeviceOrientation

Source: `packages/contracts/src/device.ts:311`.

```text
Enum([
  "portrait",
  "landscape_left",
  "portrait_upside_down",
  "landscape_right",
])
```

### DevicePermission

Source: `packages/contracts/src/device.ts:352`.

```text
Enum([
  "camera",
  "microphone",
  "photos",
  "contacts",
  "calendar",
  "reminders",
  "location",
  "notifications",
  "motion",
  "media-library",
  "faceid",
])
```

### DevicePlatform

Source: `packages/contracts/src/device.ts:7`.

```text
Enum(["ios", "android"])
```

### DevicePlatformAvailability

Source: `packages/contracts/src/device.ts:127`.

```text
Object({
  platform: DevicePlatform,
  available: Boolean,
  reason: Optional(String),
})
```

### DevicePlatformUnavailableError

Source: `packages/contracts/src/device.ts:461`.

```text
Object({_tag: Const("DevicePlatformUnavailableError"),
    hostId: DeviceHostId,
    platform: DevicePlatform,
    reason: String,
  })
```

### DeviceServiceState

Source: `packages/contracts/src/device.ts:230`.

```text
DeviceServiceStateWire
```

### DeviceServiceStateDecoded

Source: `packages/contracts/src/device.ts:225`.

```text
Object({
  ...deviceServiceStateFields,
  hosts: Array(DeviceHostSummary),
})
```

### DeviceServiceStateWire

Source: `packages/contracts/src/device.ts:220`.

```text
Object({
  ...deviceServiceStateFields,
  hosts: Array(MachineDeviceHostSummary),
  easHosts: Optional(Array(EasDeviceHostSummary)),
})
```

### DeviceSession

Source: `packages/contracts/src/device.ts:177`.

```text
Object({
  threadId: ThreadId,
  hostId: DeviceHostId,
  deviceId: DeviceId,
  platform: DevicePlatform,
  openedAt: String,
})
```

### DeviceSettings

Source: `packages/contracts/src/device.ts:319`.

```text
Object({
  appearance: Optional(DeviceAppearance),
  textSize: Optional(DeviceTextSize),
  reduceMotion: Optional(Boolean),
  increaseContrast: Optional(Boolean),
  reduceTransparency: Optional(Boolean),
  showBorders: Optional(Boolean),
  voiceOver: Optional(Boolean),
  liquidGlass: Optional(Enum(["clear", "tinted"])),
  colorFilter: Optional(DeviceColorFilter),
  networkEnabled: Optional(Boolean),
  location: Optional(
    Nullable(Object({ latitude: Number, longitude: Number }),
  ),
})
```

### DeviceShutdownInput

Source: `packages/contracts/src/device.ts:289`.

```text
Object({
  hostId: Optional(DeviceHostId),
  deviceId: DeviceId,
  platform: DevicePlatform,
})
```

### DeviceSummary

Source: `packages/contracts/src/device.ts:116`.

```text
Object({
  hostId: DeviceHostId,
  id: DeviceId,
  platform: DevicePlatform,
  name: TrimmedNonEmptyString,
  version: String,
  booted: Boolean,
  physical: Boolean,
})
```

### DeviceTarget

Source: `packages/contracts/src/device.ts:367`.

```text
{
  hostId: Optional(DeviceHostId),
  deviceId: DeviceId,
}
```

### DeviceTextSize

Source: `packages/contracts/src/device.ts:299`.

```text
Enum(["small", "default", "large", "extra-large"])
```

### DeviceToolVersion

Source: `packages/contracts/src/device.ts:134`.

```text
Object({
  requiredVersion: String,
  installedVersions: Array(String),
  runningVersion: Nullable(String),
})
```

### DeviceToolVersions

Source: `packages/contracts/src/device.ts:141`.

```text
Object({
  hub: DeviceToolVersion,
  agent: DeviceToolVersion,
})
```

### DiscoveredLocalServer

Source: `packages/contracts/src/preview.ts:315`.

```text
Object({
  host: TrimmedNonEmptyString,
  port: Int.check(isGreaterThan(0)).check(isLessThan(65536)),
  url: Url,
  processName: Nullable(TrimmedNonEmptyString),
  pid: Nullable(Int.check(isGreaterThan(0))),
  terminal: Nullable(
    Object({
      threadId: ThreadId,
      terminalId: TrimmedNonEmptyString,
    }),
})
```

### DiscoveredLocalServerList

Source: `packages/contracts/src/preview.ts:330`.

```text
Object({
  servers: Array(DiscoveredLocalServer),
  scannedAt: String,
  configuredUrlProbing: Optional(Const(true)),
})
```

### DpopFailureReason

Source: `packages/contracts/src/baseSchemas.ts:35`.

```text
Enum([
  "time_window",
  "key_mismatch",
  "request_mismatch",
  "token_mismatch",
  "replay",
  "invalid_proof",
])
```

### EDITORS

Source: `packages/contracts/src/editor.ts:17`.

```text
[
  {
    id: "cursor",
    label: "Cursor",
    commands: ["cursor"],
    baseArgs: ["--classic"],
    launchStyle: "goto",
    remoteScheme: "cursor",
  },
  { id: "trae", label: "Trae", commands: ["trae"], launchStyle: "goto" },
  { id: "kiro", label: "Kiro", commands: ["kiro"], baseArgs: ["ide"], launchStyle: "goto" },
  {
    id: "vscode",
    label: "VS Code",
    commands: ["code"],
    launchStyle: "goto",
    remoteScheme: "vscode",
  },
  {
    id: "vscode-insiders",
    label: "VS Code Insiders",
    commands: ["code-insiders"],
    launchStyle: "goto",
    remoteScheme: "vscode-insiders",
  },
  {
    id: "vscodium",
    label: "VSCodium",
    commands: ["codium"],
    launchStyle: "goto",
    remoteScheme: "vscodium",
  },
  {
    id: "zed",
    label: "Zed",
    commands: ["zed", "zeditor"],
    launchStyle: "direct-path",
    remoteScheme: "zed",
  },
  {
    id: "antigravity",
    label: "Antigravity",
    commands: ["antigravity-ide", "agy-ide"],
    launchStyle: "goto",
  },
  {
    id: "idea",
    label: "IntelliJ IDEA",
    commands: ["idea"],
    launchStyle: "line-column",
    jetbrainsProductCode: "IU",
  },
  {
    id: "aqua",
    label: "Aqua",
    commands: ["aqua"],
    launchStyle: "line-column",
    jetbrainsProductCode: "QA",
  },
  {
    id: "clion",
    label: "CLion",
    commands: ["clion"],
    launchStyle: "line-column",
    jetbrainsProductCode: "CL",
  },
  {
    id: "datagrip",
    label: "DataGrip",
    commands: ["datagrip"],
    launchStyle: "line-column",
    jetbrainsProductCode: "DB",
  },
  {
    id: "dataspell",
    label: "DataSpell",
    commands: ["dataspell"],
    launchStyle: "line-column",
    jetbrainsProductCode: "DS",
  },
  {
    id: "goland",
    label: "GoLand",
    commands: ["goland"],
    launchStyle: "line-column",
    jetbrainsProductCode: "GO",
  },
  {
    id: "phpstorm",
    label: "PhpStorm",
    commands: ["phpstorm"],
    launchStyle: "line-column",
    jetbrainsProductCode: "PS",
  },
  {
    id: "pycharm",
    label: "PyCharm",
    commands: ["pycharm"],
    launchStyle: "line-column",
    jetbrainsProductCode: "PY",
  },
  {
    id: "rider",
    label: "Rider",
    commands: ["rider"],
    launchStyle: "line-column",
    jetbrainsProductCode: "RD",
  },
  {
    id: "rubymine",
    label: "RubyMine",
    commands: ["rubymine"],
    launchStyle: "line-column",
    jetbrainsProductCode: "RM",
  },
  {
    id: "rustrover",
    label: "RustRover",
    commands: ["rustrover"],
    launchStyle: "line-column",
    jetbrainsProductCode: "RR",
  },
  {
    id: "webstorm",
    label: "WebStorm",
    commands: ["webstorm"],
    launchStyle: "line-column",
    jetbrainsProductCode: "WS",
  },
  { id: "file-manager", label: "File Manager", commands: null, launchStyle: "direct-path" },
]  satisfies ReadonlyArray<EditorDefinition>
```

### ENVIRONMENT_MACHINE_KINDS

Source: `packages/contracts/src/environment.ts:29`.

```text
[
  "server",
  "cloud",
  "linux",
  "desktop",
  "laptop",
  "mac-mini",
  "mac-studio",
]
```

### ENVIRONMENT_VARIABLE_NAME_MAX_CHARS

Source: `packages/contracts/src/providerInstance.ts:7`.

```text
128
```

### EasDeviceHostConfig

Source: `packages/contracts/src/device.ts:36`.

```text
Object({
  kind: Const("eas"),
  id: ConfiguredDeviceHostId,
  label: TrimmedNonEmptyString,
  appId: Optional(TrimmedNonEmptyString),
  accessToken: TrimmedString,
  maxSessionMinutes: Int.check(isBetween({ minimum: 1, maximum: 115 }),
  idleMinutes: Int.check(isBetween({ minimum: 1, maximum: 60 }),
  maxConcurrentSessions: Int.check(isBetween({ minimum: 1, maximum: 20 }),
}).check(
  Predicate('Idle minutes must be smaller than session minutes.'),
)
```

### EasDeviceHostConfigs

Source: `packages/contracts/src/device.ts:86`.

```text
Array(EasDeviceHostConfig).check(uniqueHostIds)
```

### EasDeviceHostSummary

Source: `packages/contracts/src/device.ts:160`.

```text
Object({
  ...deviceHostSummaryFields,
  kind: Const("eas"),
})
```

### EditorId

Source: `packages/contracts/src/editor.ts:151`.

```text
Enum(EDITORS.map((e) => e.id))
```

### ElementContextDetails

Source: `packages/contracts/src/composerContext.ts:127`.

```text
Object({
  pageUrl: ShortString,
  pageTitle: NullableShortString,
  tagName: TrimmedNonEmptyString.check(isMaxLength(255)),
  selector: NullableShortString,
  htmlPreview: BoundedString(COMPOSER_CONTEXT_ELEMENT_HTML_MAX_CHARS),
  componentName: NullableShortString,
  source: Nullable(ElementContextSource),
  styles: BoundedString(COMPOSER_CONTEXT_ELEMENT_STYLES_MAX_CHARS),
})
```

### ElementContextRecord

Source: `packages/contracts/src/composerContext.ts:139`.

```text
Object({
  ...recordBase,
  kind: Const("element"),
  ...ElementContextDetails.fields,
})
```

### ElementContextSource

Source: `packages/contracts/src/composerContext.ts:119`.

```text
Object({
  functionName: NullableShortString,
  fileName: NullableShortString,
  lineNumber: Nullable(NonNegativeInt),
  columnNumber: Nullable(NonNegativeInt),
})
```

### EnvironmentAuthInvalidError

Source: `packages/contracts/src/environmentHttp.ts:151`.

```text
Object({_tag: Const("EnvironmentAuthInvalidError"),
    code: Const("auth_invalid"),
    reason: EnvironmentAuthInvalidReason,
    dpopFailureReason: Optional(DpopFailureReason),
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentAuthInvalidReason

Source: `packages/contracts/src/environmentHttp.ts:101`.

```text
Enum([
  "missing_credential",
  "invalid_credential",
])
```

### EnvironmentAuthorizationError

Source: `packages/contracts/src/auth.ts:340`.

```text
Object({_tag: Const("EnvironmentAuthorizationError"),
    message: String,
    requiredScope: AuthEnvironmentScope,
    requiredPermission: Optional(String),
  })
```

### EnvironmentCloudEndpointUnavailableError

Source: `packages/contracts/src/environmentHttp.ts:321`.

```text
Object({_tag: Const("EnvironmentCloudEndpointUnavailableError"),
    message: String,
    endpointRuntimeStatus: Unknown,
  })
```

### EnvironmentCloudLinkStateResult

Source: `packages/contracts/src/environmentHttp.ts:495`.

```text
Object({
  linked: Boolean,
  cloudUserId: Nullable(String),
  relayUrl: Nullable(String),
  relayIssuer: Nullable(String),
  managedTunnelActive: Optional(Boolean),
  publishAgentActivity: Boolean,
  holdWebhooksWhileOffline: Optional(Boolean),
  linkRevoked: Optional(Boolean),
  reachability: Optional(EnvironmentConnectReachability),
})
```

### EnvironmentCloudPreferencesRequest

Source: `packages/contracts/src/environmentHttp.ts:509`.

```text
Object({
  publishAgentActivity: Boolean,
  holdWebhooksWhileOffline: Optional(Boolean),
})
```

### EnvironmentCloudRelayConfigResult

Source: `packages/contracts/src/environmentHttp.ts:406`.

```text
Object({
  ok: Boolean,
  endpointRuntimeStatus: Unknown,
})
```

### EnvironmentConnectReachability

Source: `packages/contracts/src/environmentHttp.ts:426`.

```text
Union([
  Object({
    status: Const("reachable"),
    checkedAt: String,
    readyConnections: Nullable(Int),
    protocol: Nullable(String),
  }),
  Object({
    status: Const("unreachable"),
    checkedAt: String,
    cause: EnvironmentConnectReachabilityCause,
    reason: String,
    hint: Nullable(String),
    fix: String,
  }),
])
```

### EnvironmentConnectReachabilityCause

Source: `packages/contracts/src/environmentHttp.ts:412`.

```text
Enum([
  "not_linked",
  "connector_not_running",
  "connector_not_ready",
  "edge_unreachable",
  "no_connector",
  "origin_unreachable",
  "dns_failed",
  "timeout",
  "unexpected_response",
])
```

### EnvironmentHttpBadRequestError

Source: `packages/contracts/src/environmentHttp.ts:261`.

```text
Object({_tag: Const("EnvironmentHttpBadRequestError"),
    message: String,
  })
```

### EnvironmentHttpCloudErrors

Source: `packages/contracts/src/environmentHttp.ts:397`.

```text
[
  EnvironmentHttpBadRequestError,
  EnvironmentHttpUnauthorizedError,
  EnvironmentHttpForbiddenError,
  EnvironmentHttpConflictError,
  EnvironmentHttpInternalServerError,
  EnvironmentScopeRequiredError,
]
```

### EnvironmentHttpCommonError

Source: `packages/contracts/src/environmentHttp.ts:246`.

```text
Union([
  EnvironmentRequestInvalidError,
  EnvironmentAuthInvalidError,
  EnvironmentScopeRequiredError,
  EnvironmentOperationForbiddenError,
  EnvironmentResourceNotFoundError,
  EnvironmentInternalError,
])
```

### EnvironmentHttpConflictError

Source: `packages/contracts/src/environmentHttp.ts:309`.

```text
Object({_tag: Const("EnvironmentHttpConflictError"),
    message: String,
  })
```

### EnvironmentHttpForbiddenError

Source: `packages/contracts/src/environmentHttp.ts:285`.

```text
Object({_tag: Const("EnvironmentHttpForbiddenError"),
    message: String,
  })
```

### EnvironmentHttpInternalServerError

Source: `packages/contracts/src/environmentHttp.ts:297`.

```text
Object({_tag: Const("EnvironmentHttpInternalServerError"),
    message: String,
  })
```

### EnvironmentHttpUnauthorizedError

Source: `packages/contracts/src/environmentHttp.ts:273`.

```text
Object({_tag: Const("EnvironmentHttpUnauthorizedError"),
    message: String,
  })
```

### EnvironmentInternalError

Source: `packages/contracts/src/environmentHttp.ts:207`.

```text
Object({_tag: Const("EnvironmentInternalError"),
    code: Const("internal_error"),
    reason: EnvironmentInternalErrorReason,
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentInternalErrorReason

Source: `packages/contracts/src/environmentHttp.ts:112`.

```text
Enum([
  "bootstrap_validation_failed",
  "browser_session_issuance_failed",
  "browser_session_cookie_failed",
  "access_token_issuance_failed",
  "websocket_ticket_issuance_failed",
  "pairing_credential_issuance_failed",
  "pairing_links_load_failed",
  "pairing_link_revoke_failed",
  "client_sessions_load_failed",
  "client_session_revoke_failed",
  "project_snapshot_failed",
  "project_mutation_failed",
  "orchestration_snapshot_failed",
  "orchestration_thread_snapshot_failed",
  "orchestration_thread_bounded_snapshot_failed",
  "orchestration_thread_history_failed",
  "internal_error",
])
```

### EnvironmentMachineKind

Source: `packages/contracts/src/environment.ts:38`.

```text
Enum(ENVIRONMENT_MACHINE_KINDS)
```

### EnvironmentOperationForbiddenError

Source: `packages/contracts/src/environmentHttp.ts:189`.

```text
Object({_tag: Const("EnvironmentOperationForbiddenError"),
    code: Const("operation_forbidden"),
    reason: EnvironmentOperationForbiddenReason,
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentOperationForbiddenReason

Source: `packages/contracts/src/environmentHttp.ts:107`.

```text
Enum([
  "current_session_revoke_not_allowed",
])
```

### EnvironmentOrchestrationShellSnapshotQuery

Source: `packages/contracts/src/environmentHttp.ts:670`.

```text
Object({
  omitIdleSubagents: Optional(String),
  omitOlderSettled: Optional(String),
})
```

### EnvironmentOrchestrationThreadBoundedSnapshotQuery

Source: `packages/contracts/src/environmentHttp.ts:675`.

```text
Object({
  compactTurnItems: Optional(String),
  dedupeBodies: Optional(String),
  compactIds: Optional(String),
})
```

### EnvironmentOrchestrationThreadHistoryQuery

Source: `packages/contracts/src/environmentHttp.ts:681`.

```text
Object({
  cursor: TrimmedNonEmptyString,
  throughEntryId: Optional(TrimmedNonEmptyString),
  view: Optional(Enum(["conversation", "activity"])),
  compactIds: Optional(String),
})
```

### EnvironmentRequestInvalidError

Source: `packages/contracts/src/environmentHttp.ts:133`.

```text
Object({_tag: Const("EnvironmentRequestInvalidError"),
    code: Const("invalid_request"),
    reason: EnvironmentRequestInvalidReason,
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentRequestInvalidReason

Source: `packages/contracts/src/environmentHttp.ts:92`.

```text
Enum([
  "invalid_scope",
  "scope_not_granted",
  "invalid_command",
  "invalid_history_cursor",
  "thread_transcript_too_large",
])
```

### EnvironmentResourceNotFoundError

Source: `packages/contracts/src/environmentHttp.ts:228`.

```text
Object({_tag: Const("EnvironmentResourceNotFoundError"),
    code: Const("not_found"),
    reason: EnvironmentResourceNotFoundReason,
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentResourceNotFoundReason

Source: `packages/contracts/src/environmentHttp.ts:225`.

```text
Enum(["thread_not_found"])
```

### EnvironmentScopeRequiredError

Source: `packages/contracts/src/environmentHttp.ts:170`.

```text
Object({_tag: Const("EnvironmentScopeRequiredError"),
    code: Const("insufficient_scope"),
    requiredScope: AuthEnvironmentScope,
    requiredPermission: Optional(String),
    traceId: TrimmedNonEmptyString,
  })
```

### EnvironmentTheme

Source: `packages/contracts/src/server.ts:546`.

```text
Object({
  id: EnvironmentThemeId,
  ...environmentThemeFields,
})
```

### EnvironmentThemeColor

Source: `packages/contracts/src/server.ts:511`.

```text
String.check(
  isPattern(/^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/),
)
```

### EnvironmentThemeColors

Source: `packages/contracts/src/server.ts:522`.

```text
Record(
  String.check(isPattern(/^[a-zA-Z][a-zA-Z0-9]{0,63}$/)),
  TrimmedNonEmptyString.check(isMaxLength(64)),
)
```

### EnvironmentThemeId

Source: `packages/contracts/src/server.ts:517`.

```text
String.check(
  isPattern(/^(?!(?:system|light|dark)$)[a-z0-9](?:[a-z0-9-]{0,47})$/),
)
```

### ExecutionEnvironmentCapabilities

Source: `packages/contracts/src/environment.ts:72`.

```text
Object({
  repositoryIdentity: Boolean,
  connectionProbe: Optional(Boolean),
  attachmentUploads: Optional(Boolean),
  questionAttachments: Optional(Boolean),
  fileAttachments: Optional(
    Object({
      maxUploadBytes: Int.check(isGreaterThanOrEqualTo(1)),
    }),
  pullRequests: Optional(Boolean),
  pullRequestChecks: Optional(Boolean),
  inlineMessageContext: Optional(Boolean),
  requiredWorktreeBootstrap: Optional(Boolean),
  threadSettlement: Optional(Boolean),
  threadAutoSettlement: Optional(Boolean),
  storageCleanup: Optional(Boolean),
  storageCleanupRun: Optional(Boolean),
  databaseStorage: Optional(Boolean),
  otlpEndpointCheck: Optional(Boolean),
  projectWorktreeCleanup: Optional(Boolean),
  settledWorktreeCleanup: Optional(Boolean),
  worktreesDirectory: Optional(Boolean),
  threadAutoSettlementScope: Optional(Boolean),
  threadRestartContinuation: Optional(Boolean),
  projectSettingsOverrides: Optional(Boolean),
  projectProviderInstanceOverrides: Optional(Boolean),
  threadSnooze: Optional(Boolean),
  environmentThemes: Optional(Boolean),
  usageLimitSources: Optional(Boolean),
  providerWorkspaceSnapshotsOnDemand: Optional(Boolean),
  usagePriceOverrides: Optional(Boolean),
  usageModelAliases: Optional(Boolean),
  threadPinning: Optional(Boolean),
  threadPinReorder: Optional(Boolean),
  threadActiveReorder: Optional(Boolean),
  threadAutoSettleOptOut: Optional(Boolean),
  threadTitleRegeneration: Optional(Boolean),
  threadVisitedTracking: Optional(Boolean),
  threadPullRequestLinking: Optional(Boolean),
  serverResolvedCommandContext: Optional(Boolean),
  threadStop: Optional(Boolean),
  threadPullRequests: Optional(Boolean),
  threadPullRequestWatch: Optional(Boolean),
  pullRequestStackActions: Optional(Boolean),
  serverSelfUpdate: Optional(ServerSelfUpdateCapability),
  serverInstallation: ForwardCompatibleOptional(ServerInstallation),
  serverSelfUpdateProgress: Optional(Boolean),
  serverUpdateThreadContinuation: Optional(Boolean),
  agentActivityPublishing: Optional(Boolean),
  projectCloneTracking: Optional(Boolean),
  ignoredFileListing: Optional(Boolean),
  environmentIcon: Optional(Boolean),
  desktopAppUpdate: Optional(Boolean),
  commandOutputStreaming: Optional(Boolean),
  serverBrowser: Optional(Boolean),
})
```

### ExecutionEnvironmentDescriptor

Source: `packages/contracts/src/environment.ts:140`.

```text
Object({
  environmentId: EnvironmentId,
  label: TrimmedNonEmptyString,
  platform: ExecutionEnvironmentPlatform,
  serverVersion: TrimmedNonEmptyString,
  orchestrationProtocolVersion: Optional(Int),
  capabilities: ExecutionEnvironmentCapabilities,
})
```

### ExecutionEnvironmentPlatform

Source: `packages/contracts/src/environment.ts:42`.

```text
Object({
  os: ExecutionEnvironmentPlatformOs,
  arch: ExecutionEnvironmentPlatformArch,
  machine: ForwardCompatibleOptional(EnvironmentMachineKind),
})
```

### ExecutionEnvironmentPlatformArch

Source: `packages/contracts/src/environment.ts:26`.

```text
Enum(["arm64", "x64", "other"])
```

### ExecutionEnvironmentPlatformOs

Source: `packages/contracts/src/environment.ts:18`.

```text
Enum([
  "darwin",
  "linux",
  "windows",
  "unknown",
])
```

### ExternalLauncherBrowserSpawnError

Source: `packages/contracts/src/editor.ts:283`.

```text
Object({_tag: Const("ExternalLauncherBrowserSpawnError"),
    ...ExternalLauncherSpawnFields,
    target: String,
  })
```

### ExternalLauncherCommandNotFoundError

Source: `packages/contracts/src/editor.ts:254`.

```text
Object({_tag: Const("ExternalLauncherCommandNotFoundError"),
    editor: EditorId,
    command: String,
  })
```

### ExternalLauncherEditorSpawnError

Source: `packages/contracts/src/editor.ts:295`.

```text
Object({_tag: Const("ExternalLauncherEditorSpawnError"),
    ...ExternalLauncherSpawnFields,
    editor: EditorId,
    target: String,
  })
```

### ExternalLauncherError

Source: `packages/contracts/src/editor.ts:308`.

```text
Union([
  ExternalLauncherUnknownEditorError,
  ExternalLauncherUnsupportedEditorError,
  ExternalLauncherCommandNotFoundError,
  ExternalLauncherUnsupportedTargetError,
  ExternalLauncherBrowserSpawnError,
  ExternalLauncherEditorSpawnError,
])
```

### ExternalLauncherSpawnFields

Source: `packages/contracts/src/editor.ts:277`.

```text
{
  command: String,
  args: Array(String),
  cause: Defect(),
}
```

### ExternalLauncherUnknownEditorError

Source: `packages/contracts/src/editor.ts:232`.

```text
Object({_tag: Const("ExternalLauncherUnknownEditorError"),
    editor: String,
  })
```

### ExternalLauncherUnsupportedEditorError

Source: `packages/contracts/src/editor.ts:243`.

```text
Object({_tag: Const("ExternalLauncherUnsupportedEditorError"),
    editor: EditorId,
  })
```

### ExternalLauncherUnsupportedTargetError

Source: `packages/contracts/src/editor.ts:266`.

```text
Object({_tag: Const("ExternalLauncherUnsupportedTargetError"),
    editor: EditorId,
  })
```

### FILESYSTEM_METADATA_BATCH_LIMIT

Source: `packages/contracts/src/filesystem.ts:6`.

```text
64
```

### FILESYSTEM_PATH_MAX_LENGTH

Source: `packages/contracts/src/filesystem.ts:4`.

```text
512
```

### FileAttachmentCreateUploadUrlInput

Source: `packages/contracts/src/assets.ts:100`.

```text
Object({
  type: Const("file"),
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt.check(
    isGreaterThanOrEqualTo(1),
    isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_FILE_BYTES),
  ),
})
```

### FileContextRecord

Source: `packages/contracts/src/composerContext.ts:101`.

```text
Object({
  ...recordBase,
  kind: Const("file"),
  ...attachmentBinding,
})
```

### FileManagerRevealKind

Source: `packages/contracts/src/editor.ts:154`.

```text
Enum(["finder", "file-explorer", "files"])
```

### FilePath

Source: `packages/contracts/src/pullRequest.ts:664`.

```text
String.check(isNonEmpty(), isMaxLength(MAX_FILE_PATH_LENGTH))
```

### FilesystemBrowseEntry

Source: `packages/contracts/src/filesystem.ts:35`.

```text
Object({
  name: TrimmedNonEmptyString,
  fullPath: TrimmedNonEmptyString,
})
```

### FilesystemBrowseError

Source: `packages/contracts/src/filesystem.ts:59`.

```text
Object({_tag: Const("FilesystemBrowseError"),
    partialPath: Optional(TrimmedNonEmptyString),
    cwd: Optional(TrimmedNonEmptyString),
    failure: Optional(FilesystemBrowseFailure),
    parentPath: Optional(TrimmedNonEmptyString),
    platform: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### FilesystemBrowseFailure

Source: `packages/contracts/src/filesystem.ts:47`.

```text
Enum([
  "windows_path_unsupported",
  "current_project_required",
  "read_directory_failed",
])
```

### FilesystemBrowseInput

Source: `packages/contracts/src/filesystem.ts:29`.

```text
Object({
  partialPath: TrimmedNonEmptyString.check(isMaxLength(FILESYSTEM_PATH_MAX_LENGTH)),
  cwd: Optional(TrimmedNonEmptyString.check(isMaxLength(FILESYSTEM_PATH_MAX_LENGTH))),
})
```

### FilesystemBrowseResult

Source: `packages/contracts/src/filesystem.ts:41`.

```text
Object({
  parentPath: TrimmedNonEmptyString,
  entries: Array(FilesystemBrowseEntry),
})
```

### FilesystemEntryMetadata

Source: `packages/contracts/src/filesystem.ts:16`.

```text
Object({
  kind: Enum(["file", "directory", "other"]),
  byteLength: Optional(NonNegativeInt),
  mimeType: Optional(TrimmedNonEmptyString),
})
```

### FilesystemGetMetadataInput

Source: `packages/contracts/src/filesystem.ts:8`.

```text
Object({
  paths: Array(
    TrimmedNonEmptyString.check(isMaxLength(FILESYSTEM_PATH_MAX_LENGTH)),
  ).check(isMinLength(1), isMaxLength(FILESYSTEM_METADATA_BATCH_LIMIT)),
})
```

### FilesystemGetMetadataResult

Source: `packages/contracts/src/filesystem.ts:24`.

```text
Object({
  entries: Array(Nullable(FilesystemEntryMetadata)),
})
```

### ForwardCompatibleArray

Source: `packages/contracts/src/baseSchemas.ts:98`.

```text
Array(element); ignore unknown/unencodable elements at compatible decoding boundaries.
```

### ForwardCompatibleNullable

Source: `packages/contracts/src/baseSchemas.ts:64`.

```text
Nullable(value); decode unknown values to null.
```

### ForwardCompatibleOptional

Source: `packages/contracts/src/baseSchemas.ts:48`.

```text
Optional(value); ignore unknown values.
```

### ForwardCompatibleUnion

Source: `packages/contracts/src/baseSchemas.ts:151`.

```text
Union(members), discriminated by tag. Unknown tags can be skipped; a known tag must validate completely. Servers emit known members only.
```

### ForwardCompatibleUnionArray

Source: `packages/contracts/src/baseSchemas.ts:204`.

```text
Array(ForwardCompatibleUnion(members,tag)); skip unknown tagged members.
```

### GIT_LIST_BRANCHES_MAX_LIMIT

Source: `packages/contracts/src/git.ts:16`.

```text
200
```

### GUEST_MESSAGE_MAX_LENGTH

Source: `packages/contracts/src/guestShare.ts:19`.

```text
4_000
```

### GUEST_NAME_MAX_LENGTH

Source: `packages/contracts/src/guestShare.ts:20`.

```text
40
```

### GitActionFailedEvent

Source: `packages/contracts/src/git.ts:477`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("action_failed"),
  phase: Nullable(GitActionProgressPhase),
  message: TrimmedNonEmptyStringSchema,
})
```

### GitActionFinishedEvent

Source: `packages/contracts/src/git.ts:472`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("action_finished"),
  result: GitRunStackedActionResult,
})
```

### GitActionHookFinishedEvent

Source: `packages/contracts/src/git.ts:465`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("hook_finished"),
  hookName: TrimmedNonEmptyStringSchema,
  exitCode: Nullable(Int),
  durationMs: Nullable(NonNegativeInt),
})
```

### GitActionHookOutputEvent

Source: `packages/contracts/src/git.ts:458`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("hook_output"),
  hookName: Nullable(TrimmedNonEmptyStringSchema),
  stream: GitActionProgressStream,
  text: TrimmedNonEmptyStringSchema,
})
```

### GitActionHookStartedEvent

Source: `packages/contracts/src/git.ts:453`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("hook_started"),
  hookName: TrimmedNonEmptyStringSchema,
})
```

### GitActionPhaseStartedEvent

Source: `packages/contracts/src/git.ts:447`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("phase_started"),
  phase: GitActionProgressPhase,
  label: TrimmedNonEmptyStringSchema,
})
```

### GitActionProgressBase

Source: `packages/contracts/src/git.ts:436`.

```text
Object({
  actionId: TrimmedNonEmptyStringSchema,
  cwd: TrimmedNonEmptyStringSchema,
  action: GitStackedAction,
})
```

### GitActionProgressEvent

Source: `packages/contracts/src/git.ts:484`.

```text
Union([
  GitActionStartedEvent,
  GitActionPhaseStartedEvent,
  GitActionHookStartedEvent,
  GitActionHookOutputEvent,
  GitActionHookFinishedEvent,
  GitActionFinishedEvent,
  GitActionFailedEvent,
])
```

### GitActionProgressPhase

Source: `packages/contracts/src/git.ts:26`.

```text
Enum(["branch", "commit", "push", "pr"])
```

### GitActionProgressStream

Source: `packages/contracts/src/git.ts:38`.

```text
Enum(["stdout", "stderr"])
```

### GitActionStartedEvent

Source: `packages/contracts/src/git.ts:442`.

```text
Object({
  ...GitActionProgressBase.fields,
  kind: Const("action_started"),
  phases: Array(GitActionProgressPhase),
})
```

### GitBranchStepStatus

Source: `packages/contracts/src/git.ts:50`.

```text
Enum(["created", "skipped_not_requested"])
```

### GitCommandError

Source: `packages/contracts/src/git.ts:368`.

```text
Object({_tag: Const("GitCommandError"),
  operation: String,
  command: String,
  cwd: String,
  argumentCount: Optional(Number),
  exitCode: Optional(Number),
  stdoutLength: Optional(Number),
  stderrLength: Optional(Number),
  outputLength: Optional(Number),
  reason: ForwardCompatibleOptional(GitCommandFailureReason),
  detail: String,
  cause: Optional(Defect()),
})
```

### GitCommandFailureReason

Source: `packages/contracts/src/git.ts:353`.

```text
Enum([
  "authentication_failed",
  "branch_already_exists",
  "branch_checked_out_in_worktree",
  "host_key_unverified",
  "not_a_repository",
  "path_already_exists",
  "push_rejected_by_remote",
  "push_rejected_non_fast_forward",
  "ref_not_found",
  "remote_unreachable",
  "tag_would_be_clobbered",
])
```

### GitCommitStepStatus

Source: `packages/contracts/src/git.ts:40`.

```text
Enum([
  "created",
  "skipped_no_changes",
  "skipped_not_requested",
])
```

### GitFilePath

Source: `packages/contracts/src/git.ts:15`.

```text
String.check(isNonEmpty())
```

### GitManagerError

Source: `packages/contracts/src/git.ts:400`.

```text
Object({_tag: Const("GitManagerError"),
  operation: String,
  cwd: String,
  detail: String,
  cause: Optional(Defect()),
})
```

### GitManagerServiceError

Source: `packages/contracts/src/git.ts:427`.

```text
Union([
  GitManagerError,
  GitPullRequestMaterializationError,
  GitCommandError,
  SourceControlProviderError,
  TextGenerationError,
])
```

### GitPrStepStatus

Source: `packages/contracts/src/git.ts:51`.

```text
Enum(["created", "opened_existing", "skipped_not_requested"])
```

### GitPreparePullRequestThreadInput

Source: `packages/contracts/src/git.ts:166`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  reference: GitPullRequestReference,
  mode: GitPreparePullRequestThreadMode,
  threadId: Optional(ThreadId),
})
```

### GitPreparePullRequestThreadMode

Source: `packages/contracts/src/git.ts:55`.

```text
Enum(["local", "worktree"])
```

### GitPreparePullRequestThreadResult

Source: `packages/contracts/src/git.ts:300`.

```text
Object({
  pullRequest: GitResolvedPullRequest,
  branch: TrimmedNonEmptyStringSchema,
  worktreePath: TrimmedNonEmptyStringSchema,
  isOnPullRequestHead: Boolean,
})
```

### GitPullRequestMaterializationError

Source: `packages/contracts/src/git.ts:411`.

```text
Object({_tag: Const("GitPullRequestMaterializationError"),
    cwd: TrimmedNonEmptyStringSchema,
    pullRequestNumber: PositiveInt,
    headRepository: Nullable(TrimmedNonEmptyStringSchema),
    headBranch: TrimmedNonEmptyStringSchema,
    localBranch: TrimmedNonEmptyStringSchema,
    cause: Defect(),
  })
```

### GitPullRequestRefInput

Source: `packages/contracts/src/git.ts:160`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  reference: GitPullRequestReference,
})
```

### GitPullRequestState

Source: `packages/contracts/src/git.ts:54`.

```text
Enum(["open", "closed", "merged"])
```

### GitPushStepStatus

Source: `packages/contracts/src/git.ts:45`.

```text
Enum([
  "pushed",
  "skipped_not_requested",
  "skipped_up_to_date",
])
```

### GitResolvePullRequestResult

Source: `packages/contracts/src/git.ts:295`.

```text
Object({
  pullRequest: GitResolvedPullRequest,
})
```

### GitResolvedPullRequest

Source: `packages/contracts/src/git.ts:99`.

```text
Object({
  number: PositiveInt,
  title: TrimmedNonEmptyStringSchema,
  url: String,
  baseBranch: TrimmedNonEmptyStringSchema,
  headBranch: TrimmedNonEmptyStringSchema,
  state: GitPullRequestState,
})
```

### GitRunStackedActionInput

Source: `packages/contracts/src/git.ts:126`.

```text
Object({
  actionId: TrimmedNonEmptyStringSchema,
  cwd: TrimmedNonEmptyStringSchema,
  action: GitStackedAction,
  commitMessage: Optional(TrimmedNonEmptyStringcheck(isMaxLength(10_000))),
  featureBranch: Optional(Boolean),
  filePaths: Optional(Array(GitFilePath).check(isMinLength(1))),
  threadId: Optional(ThreadId),
  projectId: Optional(ProjectId),
})
```

### GitRunStackedActionResult

Source: `packages/contracts/src/git.ts:314`.

```text
Object({
  action: GitStackedAction,
  branch: Object({
    status: GitBranchStepStatus,
    name: Optional(TrimmedNonEmptyStringSchema),
  }),
  commit: Object({
    status: GitCommitStepStatus,
    commitSha: Optional(TrimmedNonEmptyStringSchema),
    subject: Optional(TrimmedNonEmptyStringSchema),
  }),
  push: Object({
    status: GitPushStepStatus,
    branch: Optional(TrimmedNonEmptyStringSchema),
    upstreamBranch: Optional(TrimmedNonEmptyStringSchema),
    setUpstream: Optional(Boolean),
  }),
  pr: Object({
    status: GitPrStepStatus,
    url: Optional(String),
    number: Optional(PositiveInt),
    baseBranch: Optional(TrimmedNonEmptyStringSchema),
    headBranch: Optional(TrimmedNonEmptyStringSchema),
    title: Optional(TrimmedNonEmptyStringSchema),
  }),
  toast: GitRunStackedActionToast,
})
```

### GitRunStackedActionToast

Source: `packages/contracts/src/git.ts:76`.

```text
Object({
  title: TrimmedNonEmptyStringSchema,
  description: Optional(TrimmedNonEmptyStringSchema),
  cta: GitRunStackedActionToastCta,
})
```

### GitRunStackedActionToastCta

Source: `packages/contracts/src/git.ts:60`.

```text
Union([
  Object({
    kind: Const("none"),
  }),
  Object({
    kind: Const("open_pr"),
    label: TrimmedNonEmptyStringSchema,
    url: String,
  }),
  Object({
    kind: Const("run_action"),
    label: TrimmedNonEmptyStringSchema,
    action: GitRunStackedActionToastRunAction,
  }),
])
```

### GitRunStackedActionToastRunAction

Source: `packages/contracts/src/git.ts:56`.

```text
Object({
  kind: GitStackedAction,
})
```

### GitStackedAction

Source: `packages/contracts/src/git.ts:18`.

```text
Enum([
  "commit",
  "push",
  "create_pr",
  "commit_push",
  "commit_push_pr",
])
```

### GuestMessageAttribution

Source: `packages/contracts/src/composerContext.ts:260`.

```text
Object({
  guestMessageId: GuestMessageId,
  name: TrimmedNonEmptyString.check(isMaxLength(GUEST_NAME_MAX_LENGTH)),
  text: String.check(isMaxLength(GUEST_MESSAGE_MAX_LENGTH)),
  shareId: Optional(GuestShareId),
})
```

### GuestRedeemRequest

Source: `packages/contracts/src/guestShare.ts:96`.

```text
Object({
  credential: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString.check(
    isMaxLength(GUEST_NAME_MAX_LENGTH),
    Predicate('a single line of text'),
})
```

### GuestShareCreateError

Source: `packages/contracts/src/guestShare.ts:75`.

```text
Object({_tag: Const("GuestShareCreateError"),
    reason: Enum([
      "disabled",
      "connect_unavailable",
      "thread_unavailable",
      "read_failed",
      "changed",
    ]),
  })
```

### GuestShareCreateInput

Source: `packages/contracts/src/guestShare.ts:58`.

```text
Object({ threadId: ThreadId })
```

### GuestShareCreateResult

Source: `packages/contracts/src/guestShare.ts:61`.

```text
Object({
  url: TrimmedNonEmptyString,
  share: ThreadGuestShare,
})
```

### GuestShareView

Source: `packages/contracts/src/guestStream.ts:22`.

```text
Object({
  shareId: GuestShareId,
  title: String,
  guestName: TrimmedNonEmptyString,
  expiresAt: IsoDateTime,
  messages: Array(ThreadGuestMessage),
  model: GuestThreadModel,
})
```

### GuestStreamEvent

Source: `packages/contracts/src/guestStream.ts:37`.

```text
Union([
  Object({ type: Const("view"), view: GuestShareView }),
  Object({ type: Const("thread"), item: OrchestrationV2ThreadStreamItem }),
  Object({ type: Const("ended") }),
])
```

### GuestSubmitRequest

Source: `packages/contracts/src/guestShare.ts:111`.

```text
Object({
  shareId: GuestShareId,
  text: TrimmedString.check(isMaxLength(GUEST_MESSAGE_MAX_LENGTH)),
})
```

### GuestThreadModel

Source: `packages/contracts/src/guestStream.ts:14`.

```text
Object({
  driver: Nullable(ProviderDriverKind),
  name: TrimmedNonEmptyString,
  shortName: Optional(TrimmedNonEmptyString),
  subProvider: Optional(TrimmedNonEmptyString),
})
```

### HostPowerSnapshot

Source: `packages/contracts/src/background.ts:34`.

```text
Object({
  source: HostPowerSource,
  idle: BackgroundBooleanState,
  idleSeconds: Nullable(Number),
  locked: BackgroundBooleanState,
  suspended: Boolean,
  onBattery: BackgroundBooleanState,
  lowPowerMode: BackgroundBooleanState,
  thermalState: HostPowerThermalState,
  stale: Boolean,
  updatedAt: DateTimeUtc,
})
```

### HostPowerSource

Source: `packages/contracts/src/background.ts:24`.

```text
Enum([
  "unknown",
  "node-macos-shell",
  "node-macos-native",
  "node-linux",
  "node-windows",
  "electron-main",
])
```

### HostPowerThermalState

Source: `packages/contracts/src/background.ts:15`.

```text
Enum([
  "unknown",
  "nominal",
  "fair",
  "serious",
  "critical",
])
```

### HostResourcesSnapshot

Source: `packages/contracts/src/resourceTelemetry.ts:9`.

```text
Object({
  sampledAt: NonNegativeInt,
  cpuUtilization: Nullable(Number.check(isBetween({ minimum: 0, maximum: 1 })),
  cpuCount: NonNegativeInt,
  availableMemoryBytes: NonNegativeInt,
  totalMemoryBytes: NonNegativeInt,
})
```

### IDLE_COMPACTION_MODES

Source: `packages/contracts/src/settings.ts:639`.

```text
[
  { value: "onReturn", label: "When you return" },
  { value: "beforeCacheExpires", label: "Before the cache expires (beta)" },
  { value: "off", label: "Never" },
]
```

### IdTableMarker

Source: `packages/contracts/src/orchestrationV2.ts:3548`.

```text
Optional(Const(true)); internal encoder marker, omitted from JSON and replaced by ids table
```

### IdleCompactionMode

Source: `packages/contracts/src/settings.ts:644`.

```text
Enum(IDLE_COMPACTION_MODES.map((mode) => mode.value))
```

### ImageAttachmentCreateUploadUrlInput

Source: `packages/contracts/src/assets.ts:90`.

```text
Object({
  type: Optional(Const("image")),
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: Enum(PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES),
  sizeBytes: NonNegativeInt.check(
    isGreaterThanOrEqualTo(1),
    isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES),
  ),
})
```

### ImageContextRecord

Source: `packages/contracts/src/composerContext.ts:94`.

```text
Object({
  ...recordBase,
  kind: Const("image"),
  ...attachmentBinding,
})
```

### IsoDateTime

Source: `packages/contracts/src/baseSchemas.ts:45`.

```text
String
```

### KeybindingCommand

Source: `packages/contracts/src/keybindings.ts:127`.

```text
Union([
  Enum(STATIC_KEYBINDING_COMMANDS),
  SCRIPT_RUN_COMMAND_PATTERN,
])
```

### KeybindingShortcut

Source: `packages/contracts/src/keybindings.ts:154`.

```text
Object({
  key: KeybindingValue,
  metaKey: Boolean,
  ctrlKey: Boolean,
  shiftKey: Boolean,
  altKey: Boolean,
  modKey: Boolean,
})
```

### KeybindingValue

Source: `packages/contracts/src/keybindings.ts:133`.

```text
TrimmedString.check(
  isMinLength(1),
  isMaxLength(MAX_KEYBINDING_VALUE_LENGTH),
)
```

### KeybindingWhen

Source: `packages/contracts/src/keybindings.ts:138`.

```text
TrimmedString.check(
  isMinLength(1),
  isMaxLength(MAX_KEYBINDING_WHEN_LENGTH),
)
```

### KeybindingWhenNode

Source: `packages/contracts/src/keybindings.ts:173`.

```text
Union([
  Object({
    type: Const("identifier"),
    name: NonEmptyString,
  }),
  Object({
    type: Const("not"),
    node: KeybindingWhenNodeRef,
  }),
  Object({
    type: Const("and"),
    left: KeybindingWhenNodeRef,
    right: KeybindingWhenNodeRef,
  }),
  Object({
    type: Const("or"),
    left: KeybindingWhenNodeRef,
    right: KeybindingWhenNodeRef,
  }),
])
```

### KeybindingWhenNodeRef

Source: `packages/contracts/src/keybindings.ts:170`.

```text
KeybindingWhenNode (recursive reference).
```

### KeybindingsConfigError

Source: `packages/contracts/src/keybindings.ts:212`.

```text
Object({_tag: Const("KeybindingsConfigParseError"),
    configPath: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### KeybindingsInvalidEntryIssue

Source: `packages/contracts/src/server.ts:37`.

```text
Object({
  kind: Const("keybindings.invalid-entry"),
  message: TrimmedNonEmptyString,
  index: Number,
})
```

### KeybindingsMalformedConfigIssue

Source: `packages/contracts/src/server.ts:32`.

```text
Object({
  kind: Const("keybindings.malformed-config"),
  message: TrimmedNonEmptyString,
})
```

### KnownComposerContextRecord

Source: `packages/contracts/src/composerContext.ts:234`.

```text
Union([
  ImageContextRecord,
  FileContextRecord,
  TerminalContextRecord,
  ElementContextRecord,
  PreviewAnnotationContextRecord,
  ReviewCommentContextRecord,
  MentionContextRecord,
  SkillContextRecord,
  ThreadContextRecord,
])
```

### LEGACY_PREVIEW_VIEWPORT_PRESET_IDS

Source: `packages/contracts/src/preview.ts:71`.

```text
[
  "desktop-1920x1080",
  "desktop-1440x900",
  "laptop-1366x768",
  "laptop-1280x800",
  "ipad-pro-11",
  "iphone-15-pro",
  "pixel-8",
  "galaxy-s24",
]
```

### LaunchEditorInput

Source: `packages/contracts/src/editor.ts:157`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  editor: EditorId,
  reveal: Optional(Boolean),
})
```

### LegacyCompactIdleThreads

Source: `packages/contracts/src/settings.ts:649`.

```text
Union([Boolean, IdleCompactionMode])
```

### LegacyProviderOptionSelectionsObject

Source: `packages/contracts/src/model.ts:55`.

```text
Record(String, Unknown)
```

### LegacySourceControlWritingStyle

Source: `packages/contracts/src/settings.ts:844`.

```text
TrimmedString
```

### LenientThreadOpens

Source: `packages/contracts/src/orchestrationV2.ts:397`.

```text
Nullable(OrchestrationV2ThreadOpens)
```

### MAX_FILES_VIEWED_PRESSES

Source: `packages/contracts/src/pullRequest.ts:706`.

```text
500
```

### MAX_FILE_PATH_LENGTH

Source: `packages/contracts/src/pullRequest.ts:662`.

```text
4096
```

### MAX_KEYBINDINGS_COUNT

Source: `packages/contracts/src/keybindings.ts:8`.

```text
256
```

### MAX_KEYBINDING_VALUE_LENGTH

Source: `packages/contracts/src/keybindings.ts:4`.

```text
64
```

### MAX_KEYBINDING_WHEN_LENGTH

Source: `packages/contracts/src/keybindings.ts:5`.

```text
256
```

### MAX_SCRIPT_ID_LENGTH

Source: `packages/contracts/src/keybindings.ts:7`.

```text
24
```

### MAX_SIDEBAR_AUTO_SETTLE_AFTER_DAYS

Source: `packages/contracts/src/settings.ts:86`.

```text
90
```

### MAX_WEBHOOK_DELIVERY_AGE_MINUTES

Source: `packages/contracts/src/scheduledTask.ts:64`.

```text
24 * 60
```

### MAX_WEBHOOK_FILTERS

Source: `packages/contracts/src/scheduledTask.ts:91`.

```text
16
```

### MIN_SCHEDULED_TASK_INTERVAL_MS

Source: `packages/contracts/src/scheduledTask.ts:25`.

```text
60_000
```

### MIN_SIDEBAR_AUTO_SETTLE_AFTER_DAYS

Source: `packages/contracts/src/settings.ts:85`.

```text
1
```

### MODEL_PICKER_JUMP_KEYBINDING_COMMANDS

Source: `packages/contracts/src/keybindings.ts:23`.

```text
[
  "modelPicker.jump.1",
  "modelPicker.jump.2",
  "modelPicker.jump.3",
  "modelPicker.jump.4",
  "modelPicker.jump.5",
  "modelPicker.jump.6",
  "modelPicker.jump.7",
  "modelPicker.jump.8",
  "modelPicker.jump.9",
]
```

### MODEL_PICKER_KEYBINDING_COMMANDS

Source: `packages/contracts/src/keybindings.ts:51`.

```text
[
  "modelPicker.toggle",
  "modelPicker.previousProvider",
  "modelPicker.nextProvider",
  ...MODEL_PICKER_JUMP_KEYBINDING_COMMANDS,
]
```

### MachineDeviceHostSummary

Source: `packages/contracts/src/device.ts:156`.

```text
Object({
  ...deviceHostSummaryFields,
  kind: Enum(["local", "ssh"]),
})
```

### McpAppCallToolInput

Source: `packages/contracts/src/mcpApps.ts:11`.

```text
Object({
  ...McpAppTarget,
  name: TrimmedNonEmptyString,
  arguments: Record(String, Unknown),
})
```

### McpAppCallToolResult

Source: `packages/contracts/src/mcpApps.ts:18`.

```text
Object({
  content: Array(Unknown),
  structuredContent: Optional(Unknown),
  isError: Optional(Boolean),
  _meta: Optional(Unknown),
})
```

### McpAppReadResourceInput

Source: `packages/contracts/src/mcpApps.ts:40`.

```text
Object({
  ...McpAppTarget,
  uri: TrimmedNonEmptyString,
})
```

### McpAppReadResourceResult

Source: `packages/contracts/src/mcpApps.ts:46`.

```text
Object({
  contents: Array(Unknown),
})
```

### McpAppRequestError

Source: `packages/contracts/src/mcpApps.ts:70`.

```text
Object({_tag: Const("McpAppRequestError"),
    threadId: ThreadId,
    reason: McpAppRequestErrorReason,
    cause: Optional(Defect()),
  })
```

### McpAppRequestErrorReason

Source: `packages/contracts/src/mcpApps.ts:60`.

```text
Enum([
  "not-an-app",
  "provider-unsupported",
  "session-stopped",
  "tool-not-callable",
  "unsupported-content",
  "request-failed",
])
```

### McpAppTarget

Source: `packages/contracts/src/mcpApps.ts:6`.

```text
{
  threadId: ThreadId,
  itemId: TurnItemId,
}
```

### McpAppToolInfo

Source: `packages/contracts/src/mcpApps.ts:32`.

```text
Object({
  callable: Boolean,
  readOnly: Boolean,
  title: Optional(String),
  tool: Optional(Unknown),
})
```

### McpAppToolInfoInput

Source: `packages/contracts/src/mcpApps.ts:26`.

```text
Object({
  ...McpAppTarget,
  name: TrimmedNonEmptyString,
})
```

### McpAppUpdateModelContextInput

Source: `packages/contracts/src/mcpApps.ts:52`.

```text
Object({
  ...McpAppTarget,
  conversationThreadId: ThreadId,
  content: Optional(Array(Unknown)),
  structuredContent: Optional(Record(String, Unknown)),
})
```

### MentionContextRecord

Source: `packages/contracts/src/composerContext.ts:191`.

```text
Object({
  ...recordBase,
  kind: Const("mention"),
  path: TrimmedNonEmptyString.check(isMaxLength(2_048)),
})
```

### ModelCapabilities

Source: `packages/contracts/src/model.ts:100`.

```text
Object({
  optionDescriptors: Optional(Array(ProviderOptionDescriptor)),
})
```

### ModelFavorite

Source: `packages/contracts/src/settings.ts:292`.

```text
Object({
  provider: ProviderInstanceId,
  model: TrimmedNonEmptyString,
})
```

### ModelFavorites

Source: `packages/contracts/src/settings.ts:296`.

```text
Array(ModelFavorite)
```

### ModelSelection

Source: `packages/contracts/src/modelSelection.ts:22`.

```text
ModelSelectionSource
```

### ModelSelectionPatch

Source: `packages/contracts/src/settings.ts:1418`.

```text
Object({
  instanceId: Optional(ProviderInstanceId),
  model: Optional(TrimmedNonEmptyString),
  options: Optional(ProviderOptionSelections),
})
```

### ModelSelectionSource

Source: `packages/contracts/src/modelSelection.ts:15`.

```text
Object({
  provider: Optional(Unknown),
  instanceId: Optional(Unknown),
  model: Unknown,
  options: Optional(Unknown),
})
```

### ModelSelectionWire

Source: `packages/contracts/src/modelSelection.ts:9`.

```text
Object({
  instanceId: ProviderInstanceId,
  model: TrimmedNonEmptyString,
  options: Optional(ProviderOptionSelections),
})
```

### NonNegativeInt

Source: `packages/contracts/src/baseSchemas.ts:31`.

```text
Int.check(isGreaterThanOrEqualTo(0))
```

### NullableShortString

Source: `packages/contracts/src/composerContext.ts:68`.

```text
Nullable(ShortString)
```

### ObservabilitySettings

Source: `packages/contracts/src/settings.ts:819`.

```text
Object({
  otlpTracesUrl: TrimmedString,
  otlpMetricsUrl: TrimmedString,
  otlpLogsUrl: TrimmedString,
})
```

### OmittedWhenNull

Source: `packages/contracts/src/baseSchemas.ts:78`.

```text
Optional(value) on wire; null in the decoded model encodes as an absent property.
```

### OrchestrationDispatchCommandError

Source: `packages/contracts/src/orchestrationDispatch.ts:5`.

```text
Object({_tag: Const("OrchestrationDispatchCommandError"),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
    bootstrapThreadDisposition: Optional(Enum(["deleted", "not-created"])),
  })
```

### OrchestrationGetFullThreadDiffError

Source: `packages/contracts/src/checkpointDiff.ts:59`.

```text
Object({_tag: Const("OrchestrationGetFullThreadDiffError"),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### OrchestrationGetFullThreadDiffInput

Source: `packages/contracts/src/checkpointDiff.ts:41`.

```text
Object({
  threadId: ThreadId,
  toTurnCount: NonNegativeInt,
  ignoreWhitespace: Optional(Boolean),
})
```

### OrchestrationGetTurnDiffError

Source: `packages/contracts/src/checkpointDiff.ts:51`.

```text
Object({_tag: Const("OrchestrationGetTurnDiffError"),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### OrchestrationGetTurnDiffInput

Source: `packages/contracts/src/checkpointDiff.ts:29`.

```text
TurnCountRange.mapFields(
  Overlay({
    threadId: ThreadId,
    ignoreWhitespace: Optional(Boolean),
  }),
  { unsafePreserveChecks: true },
)
```

### OrchestrationGetWorkflowScriptError

Source: `packages/contracts/src/orchestrationV2.ts:3815`.

```text
Object({_tag: Const("OrchestrationGetWorkflowScriptError"),
    reason: Enum([
      "invalid-path",
      "root-unavailable",
      "not-found",
      "outside-root",
      "not-js",
      "not-regular-file",
      "changed-during-read",
      "read-failed",
    ]),
    scriptPath: String,
    cause: Optional(Defect()),
  })
```

### OrchestrationMessageContext

Source: `packages/contracts/src/composerContext.ts:270`.

```text
Object({
  version: Const(1),
  guest: Optional(GuestMessageAttribution),
  records: Array(Unknown)
    .check(
      isMaxLength(COMPOSER_CONTEXT_MAX_RECORDS),
      Predicate('semantic validation; see constraints in sections 2, 5 and 7')
    .check(
      Predicate('semantic validation; see constraints in sections 2, 5 and 7'),
    ),
})
```

### OrchestrationProjectShell

Source: `packages/contracts/src/orchestrationProject.ts:8`.

```text
Object({
  id: ProjectId,
  title: TrimmedNonEmptyString,
  workspaceRoot: TrimmedNonEmptyString,
  repositoryIdentity: Optional(Nullable(RepositoryIdentity)),
  defaultModelSelection: Nullable(ModelSelection),
  defaultThreadEnvMode: Optional(Nullable(ThreadEnvMode)),
  autoPull: Optional(Boolean),
  faviconPath: Optional(Nullable(TrimmedNonEmptyString)),
  projectIcon: Optional(Nullable(ReceivedProjectIcon)),
  scripts: Array(ProjectScript),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
})
```

### OrchestrationSearchThreadsError

Source: `packages/contracts/src/threadSearch.ts:54`.

```text
Object({_tag: Const("OrchestrationSearchThreadsError"),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### OrchestrationSearchThreadsInput

Source: `packages/contracts/src/threadSearch.ts:20`.

```text
Object({
  query: TrimmedString.check(
    isMinLength(2),
    isMaxLength(THREAD_SEARCH_QUERY_MAX_LENGTH),
  ),
  limit: Optional(Int.check(isBetween({ minimum: 1, maximum: 50 })),
})
```

### OrchestrationSearchThreadsResult

Source: `packages/contracts/src/threadSearch.ts:49`.

```text
Object({
  matches: Array(OrchestrationThreadSearchMatch),
})
```

### OrchestrationThreadSearchMatch

Source: `packages/contracts/src/threadSearch.ts:29`.

```text
Object({
  threadId: ThreadId,
  projectId: ProjectId,
  source: OrchestrationThreadSearchSource,
  snippet: String.check(isMaxLength(240)),
  messageCreatedAt: Nullable(IsoDateTime),
  quality: Optional(Finite),
  hits: Optional(Int),
  messageId: Optional(MessageId),
})
```

### OrchestrationThreadSearchSource

Source: `packages/contracts/src/threadSearch.ts:12`.

```text
Enum(["user", "assistant"])
```

### OrchestrationV2Actor

Source: `packages/contracts/src/orchestrationV2.ts:86`.

```text
Enum(["user", "agent", "system"])
```

### OrchestrationV2AppThread

Source: `packages/contracts/src/orchestrationV2.ts:401`.

```text
Object({
  ...OrchestrationV2CreationFields,
  id: ThreadId,
  projectId: ProjectId,
  title: TrimmedNonEmptyString,
  providerInstanceId: ProviderInstanceId,
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  branch: Nullable(TrimmedNonEmptyString),
  worktreePath: Nullable(TrimmedNonEmptyString),
  linkedPullRequest: Optional(Nullable(ThreadLinkedPullRequest)),
  pullRequests: Optional(Array(ThreadPullRequestLink)),
  branchPullRequest: Optional(Nullable(ThreadLinkedPullRequest)),
  activeProviderThreadId: Nullable(ProviderThreadId),
  historyOrigin: Optional(OrchestrationV2ThreadHistoryOrigin),
  lineage: OrchestrationV2AppThreadLineage,
  forkedFrom: Nullable(
    Union([
      Object({ type: Const("run"), threadId: ThreadId, runId: RunId }),
      Object({ type: Const("node"), nodeId: NodeId }),
      Object({
        type: Const("provider_thread"),
        providerThreadId: ProviderThreadId,
        providerTurnId: Optional(ProviderTurnId),
      }),
    ]),
  ),
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
  archivedAt: Nullable(DateTimeUtc),
  settledOverride: Nullable(Enum(["settled", "active"])),
  settledAt: Nullable(DateTimeUtc),
  unsettledAt: Optional(Nullable(DateTimeUtc)),
  snoozedUntil: Optional(Nullable(DateTimeUtc)),
  snoozedAt: Optional(Nullable(DateTimeUtc)),
  lastSnoozeWakeAt: Optional(Nullable(DateTimeUtc)),
  limitRecovery: Optional(Nullable(OrchestrationV2LimitRecovery)),
  guestShare: Optional(Nullable(ThreadGuestShare)),
  pinnedAt: Optional(Nullable(DateTimeUtc)),
  autoSettleDisabledAt: Optional(Nullable(DateTimeUtc)),
  pinOrderKey: Optional(Nullable(TrimmedNonEmptyString)),
  activeOrderKey: Optional(Nullable(TrimmedNonEmptyString)),
  lastVisitedAt: Nullable(DateTimeUtc),
  opens: Optional(LenientThreadOpens),
  titleRegeneration: Optional(
    Nullable(
      Object({
        requestId: CommandId,
        startedAt: DateTimeUtc,
      }),
  ),
  rollbackRequestId: Optional(CommandId),
  rollbackFailure: Optional(
    Nullable(
      Object({
        requestId: CommandId,
        message: TrimmedNonEmptyString,
      }),
  ),
  deletedAt: Nullable(DateTimeUtc),
})
```

### OrchestrationV2AppThreadJson

Source: `packages/contracts/src/orchestrationV2.ts:2122`.

```text
MergeFields(OrchestrationV2AppThread, {
  createdAt: DateTimeUtcFromString,
  updatedAt: DateTimeUtcFromString,
  archivedAt: Nullable(DateTimeUtcFromString),
  settledAt: Nullable(DateTimeUtcFromString),
  unsettledAt: Optional(Nullable(DateTimeUtcFromString)),
  snoozedUntil: Optional(Nullable(DateTimeUtcFromString)),
  snoozedAt: Optional(Nullable(DateTimeUtcFromString)),
  lastSnoozeWakeAt: Optional(Nullable(DateTimeUtcFromString)),
  pinnedAt: Optional(Nullable(DateTimeUtcFromString)),
  autoSettleDisabledAt: Optional(Nullable(DateTimeUtcFromString)),
  lastVisitedAt: Nullable(DateTimeUtcFromString),
  titleRegeneration: Optional(
    Nullable(
      Object({
        requestId: CommandId,
        startedAt: DateTimeUtcFromString,
      }),
  ),
  deletedAt: Nullable(DateTimeUtcFromString),
})
```

### OrchestrationV2AppThreadLineage

Source: `packages/contracts/src/orchestrationV2.ts:118`.

```text
Object({
  parentThreadId: Nullable(ThreadId),
  relationshipToParent: Nullable(Enum(["fork", "subagent"])),
  rootThreadId: ThreadId,
})
```

### OrchestrationV2ApprovalCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:272`.

```text
Object({
  supportsCommandApproval: Boolean,
  supportsFileReadApproval: Boolean,
  supportsFileChangeApproval: Boolean,
  supportsApplyPatchApproval: Boolean,
  approvalsHaveNativeRequestIds: Boolean,
  approvalCallbacksAreLiveOnly: Boolean,
  approvalsCanOriginateFromSubagents: Boolean,
})
```

### OrchestrationV2ArchivedShellSnapshot

Source: `packages/contracts/src/orchestrationV2.ts:3366`.

```text
Object({
  schemaVersion: PositiveInt,
  snapshotSequence: NonNegativeInt,
  projects: Array(OrchestrationProjectShell),
  threads: Array(OrchestrationV2ThreadShell),
})
```

### OrchestrationV2ArchivedShellStreamItem

Source: `packages/contracts/src/orchestrationV2.ts:3374`.

```text
Union([
  Object({
    kind: Const("snapshot"),
    snapshot: OrchestrationV2ArchivedShellSnapshot,
  }),
  Object({
    kind: Const("thread.updated"),
    sequence: NonNegativeInt,
    thread: OrchestrationV2ThreadShell,
  }),
  Object({
    kind: Const("thread.removed"),
    sequence: NonNegativeInt,
    threadId: ThreadId,
  }),
])
```

### OrchestrationV2Checkpoint

Source: `packages/contracts/src/orchestrationV2.ts:1286`.

```text
Object({
  id: CheckpointId,
  threadId: ThreadId,
  scopeId: CheckpointScopeId,
  runId: Nullable(RunId),
  nodeId: NodeId,
  parentCheckpointId: Nullable(CheckpointId),
  ordinalWithinScope: NonNegativeInt,
  appRunOrdinal: Nullable(PositiveInt),
  ref: CheckpointRef,
  status: Enum(["ready", "missing", "error", "stale"]),
  files: Array(OrchestrationV2CheckpointFileSummary),
  capturedAt: DateTimeUtc,
})
```

### OrchestrationV2CheckpointCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:314`.

```text
Object({
  appCanCheckpointFilesystem: Boolean,
  supportsNestedCheckpointScopes: Boolean,
  providerCanRollbackConversation: Boolean,
  providerRollbackReturnsSnapshot: Boolean,
  providerCanReadConversationSnapshot: Boolean,
})
```

### OrchestrationV2CheckpointFileSummary

Source: `packages/contracts/src/orchestrationV2.ts:1278`.

```text
Object({
  path: TrimmedNonEmptyString,
  kind: TrimmedNonEmptyString,
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
})
```

### OrchestrationV2CheckpointJson

Source: `packages/contracts/src/orchestrationV2.ts:2263`.

```text
MergeFields(OrchestrationV2Checkpoint, {
  capturedAt: DateTimeUtcFromString,
})
```

### OrchestrationV2CheckpointRollbackRequest

Source: `packages/contracts/src/orchestrationV2.ts:1302`.

```text
Object({
  scopeId: CheckpointScopeId,
  checkpointId: CheckpointId,
  requestedAt: DateTimeUtc,
})
```

### OrchestrationV2CheckpointRollbackRequestJson

Source: `packages/contracts/src/orchestrationV2.ts:2269`.

```text
MergeFields(OrchestrationV2CheckpointRollbackRequest, {
    requestedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2CheckpointScope

Source: `packages/contracts/src/orchestrationV2.ts:837`.

```text
Object({
  id: CheckpointScopeId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  nodeId: NodeId,
  parentScopeId: Nullable(CheckpointScopeId),
  providerThreadId: Nullable(ProviderThreadId),
  kind: Enum(["root_run", "subagent", "tool", "provider_thread", "manual"]),
  ordinalWithinParent: NonNegativeInt,
  advancesAppRunCount: Boolean,
  cwd: TrimmedNonEmptyString,
  createdAt: DateTimeUtc,
})
```

### OrchestrationV2CheckpointScopeJson

Source: `packages/contracts/src/orchestrationV2.ts:2184`.

```text
MergeFields(OrchestrationV2CheckpointScope, {
    createdAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2CheckpointUnavailableError

Source: `packages/contracts/src/orchestrationV2.ts:1310`.

```text
Object({_tag: Const("OrchestrationV2CheckpointUnavailableError"),
    threadId: ThreadId,
    target: String,
  })
```

### OrchestrationV2Command

Source: `packages/contracts/src/orchestrationV2.ts:2753`.

```text
Union([
  Object({
    type: Const("thread.stop"),
    commandId: CommandId,
    threadId: ThreadId,
    reason: Optional(String),
    stopDelegatedTasks: Optional(Boolean),
  }),
  Object({
    type: Const("thread.create"),
    ...OrchestrationV2CreationFields,
    commandId: CommandId,
    threadId: ThreadId,
    projectId: ProjectId,
    title: TrimmedNonEmptyString,
    modelSelection: ModelSelection,
    runtimeMode: RuntimeMode,
    interactionMode: ProviderInteractionMode,
    branch: Nullable(TrimmedNonEmptyString),
    worktreePath: Nullable(TrimmedNonEmptyString),
    importedNativeThread: Optional(
      Object({
        ref: Object({
          driver: ProviderDriverKind,
          nativeId: TrimmedNonEmptyString,
          strength: Const("strong"),
        }),
        metadata: Optional(OrchestrationV2ProviderThreadNativeMetadata),
      }),
  }),
  Object({
    type: Const("thread.archive"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.guest-share.close"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.guest-message.dismiss"),
    commandId: CommandId,
    threadId: ThreadId,
    guestMessageId: GuestMessageId,
  }),
  Object({
    type: Const("thread.unarchive"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.delete"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.settle"),
    commandId: CommandId,
    threadId: ThreadId,
    settledAt: Optional(DateTimeUtc),
  }),
  Object({
    type: Const("thread.auto-settle"),
    commandId: CommandId,
    threadId: ThreadId,
    snapshotAt: DateTimeUtc,
    settledAt: Optional(DateTimeUtc),
  }),
  Object({
    type: Const("thread.unsettle"),
    commandId: CommandId,
    threadId: ThreadId,
    reason: Const("user"),
  }),
  Object({
    type: Const("thread.snooze"),
    commandId: CommandId,
    threadId: ThreadId,
    snoozedUntil: IsoDateTime,
  }),
  Object({
    type: Const("thread.unsnooze"),
    commandId: CommandId,
    threadId: ThreadId,
    reason: Const("user"),
  }),
  Object({
    type: Const("thread.auto-settle.set"),
    commandId: CommandId,
    threadId: ThreadId,
    enabled: Boolean,
  }),
  Object({
    type: Const("thread.pin"),
    commandId: CommandId,
    threadId: ThreadId,
    orderKey: Optional(TrimmedNonEmptyString),
  }),
  Object({
    type: Const("thread.unpin"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.pin.reorder"),
    commandId: CommandId,
    threadId: ThreadId,
    orderKey: TrimmedNonEmptyString,
  }),
  Object({
    type: Const("thread.active.reorder"),
    commandId: CommandId,
    threadId: ThreadId,
    orderKey: TrimmedNonEmptyString,
  }),
  Object({
    type: Const("thread.visit"),
    commandId: CommandId,
    threadId: ThreadId,
    visitedAt: IsoDateTime,
    opened: Optional(Boolean),
    searchQuery: Optional(String.check(isMaxLength(200))),
  }),
  Object({
    type: Const("thread.mark-unread"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("thread.metadata.update"),
    commandId: CommandId,
    threadId: ThreadId,
    title: Optional(TrimmedNonEmptyString),
    regenerateTitle: Optional(Boolean),
    branch: Optional(Nullable(TrimmedNonEmptyString)),
    worktreePath: Optional(Nullable(TrimmedNonEmptyString)),
    expectedWorktreePath: Optional(Nullable(TrimmedNonEmptyString)),
    expectedBranch: Optional(Nullable(TrimmedNonEmptyString)),
    expectedEmpty: Optional(Boolean),
    limitRecovery: Optional(Nullable(OrchestrationV2LimitRecoveryUpdate)),
    linkedPullRequest: Optional(Nullable(ThreadLinkedPullRequest)),
  }),
  Object({
    type: Const("thread.pull-request.link"),
    commandId: CommandId,
    threadId: ThreadId,
    ...ThreadPullRequestKey.fields,
    url: TrimmedNonEmptyString,
    source: ThreadPullRequestLinkSource,
    repositoryWebUrl: Optional(TrimmedNonEmptyString),
  }),
  Object({
    type: Const("thread.pull-request.unlink"),
    commandId: CommandId,
    threadId: ThreadId,
    ...ThreadPullRequestKey.fields,
  }),
  Object({
    type: Const("thread.pull-request-link.sync"),
    commandId: CommandId,
    threadId: ThreadId,
    ...ThreadPullRequestKey.fields,
    snapshot: ThreadPullRequestSnapshot,
    stack: Nullable(ThreadPullRequestStack),
  }),
  Object({
    type: Const("thread.pull-request.watch"),
    commandId: CommandId,
    threadId: ThreadId,
    ...ThreadPullRequestKey.fields,
    watching: Boolean,
    link: Optional(
      Object({
        url: TrimmedNonEmptyString,
        source: ThreadPullRequestLinkSource,
        repositoryWebUrl: Optional(TrimmedNonEmptyString),
      }),
  }),
  Object({
    type: Const("thread.pull-request.sync"),
    commandId: CommandId,
    threadId: ThreadId,
    projectId: ProjectId,
    snapshotSequence: NonNegativeInt,
    expected: Object({
      workspaceRoot: TrimmedNonEmptyString,
      branch: Nullable(TrimmedNonEmptyString),
      worktreePath: Nullable(TrimmedNonEmptyString),
      linkedPullRequest: Nullable(ThreadLinkedPullRequest),
      branchPullRequest: Nullable(ThreadLinkedPullRequest),
    }),
    branchPullRequest: Nullable(ThreadLinkedPullRequest),
    linkedPullRequest: Optional(ThreadLinkedPullRequest),
  }),
  Object({
    type: Const("thread.title.regeneration.complete"),
    commandId: CommandId,
    threadId: ThreadId,
    requestId: CommandId,
    title: Optional(TrimmedNonEmptyString),
  }),
  Object({
    type: Const("thread.runtime-mode.set"),
    commandId: CommandId,
    threadId: ThreadId,
    runtimeMode: RuntimeMode,
  }),
  Object({
    type: Const("thread.interaction-mode.set"),
    commandId: CommandId,
    threadId: ThreadId,
    interactionMode: ProviderInteractionMode,
  }),
  Object({
    type: Const("thread.model-selection.set"),
    commandId: CommandId,
    threadId: ThreadId,
    modelSelection: ModelSelection,
  }),
  Object({
    type: Const("provider-session.detach"),
    commandId: CommandId,
    threadId: ThreadId,
    providerSessionId: ProviderSessionId,
    reason: Optional(String),
  }),
  Object({
    type: Const("message.dispatch"),
    notification: Optional(OrchestrationV2Notification),
    guestMessageId: Optional(GuestMessageId),
    ...OrchestrationV2CreationFields,
    scheduledTaskId: Optional(ScheduledTaskId),
    senderThreadId: Optional(ThreadId),
    idleCompaction: Optional(OrchestrationV2IdleCompaction),
    commandId: CommandId,
    threadId: ThreadId,
    messageId: MessageId,
    text: String,
    context: Optional(OrchestrationMessageContext),
    attachments: Array(ChatAttachment),
    titleSeed: Optional(TrimmedNonEmptyString),
    modelSelection: Optional(ModelSelection),
    sourcePlanRef: Optional(Object({ threadId: ThreadId, planId: PlanId }),
    restartContinuationOfRunId: Optional(RunId),
    usageLimitContinuationOfRunId: Optional(RunId),
    manualContinuationOfRunId: Optional(RunId),
    usageLimitRecoveryRequestId: Optional(CommandId),
    deliveryIntent: Optional(Enum(["auto", "steer", "restart"])),
    delegatedCompletion: Optional(
      Object({
        parentRunId: RunId,
        generation: PositiveInt,
        taskIds: Array(NodeId),
      }),
    dispatchMode: Union([
      Object({
        type: Const("defer_start"),
        workspaceStrategy: Optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
      }),
      Object({ type: Const("steer_active"), targetRunId: RunId }),
      Object({ type: Const("restart_active"), targetRunId: RunId }),
      Object({ type: Const("queue_after_active") }),
      Object({ type: Const("start_immediately") }),
    ]),
  }),
  Object({
    type: Const("prepared-run.release"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
  }),
  Object({
    type: Const("notification.delivery.accept"),
    commandId: CommandId,
    threadId: ThreadId,
    messageId: MessageId,
  }),
  Object({
    type: Const("prepared-run.progress"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    phase: Enum(["worktree", "setup"]),
  }),
  Object({
    type: Const("prepared-run.fail"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    failure: OrchestrationV2ProviderFailure,
  }),
  Object({
    type: Const("prepared-run.retry"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
  }),
  Object({
    type: Const("subagent.stop"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    subagentId: NodeId,
  }),
  Object({
    type: Const("run.interrupt"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    reason: Optional(String),
    holdQueue: Optional(Boolean),
  }),
  Object({
    type: Const("queued-message.promote-to-steer"),
    commandId: CommandId,
    threadId: ThreadId,
    queuedRunId: RunId,
    targetRunId: RunId,
  }),
  Object({
    type: Const("queue.resume"),
    commandId: CommandId,
    threadId: ThreadId,
  }),
  Object({
    type: Const("queued-run.start"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
  }),
  Object({
    type: Const("queued-run.reorder"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    beforeRunId: Nullable(RunId),
  }),
  Object({
    type: Const("queued-run.cancel"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
  }),
  Object({
    type: Const("queued-run.edit"),
    context: Optional(OrchestrationMessageContext),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    text: String,
    attachments: Optional(Array(ChatAttachment)),
  }),
  Object({
    type: Const("runtime-request.respond"),
    commandId: CommandId,
    threadId: ThreadId,
    requestId: RuntimeRequestId,
    decision: Optional(ProviderApprovalDecision),
    feedback: Optional(String),
    answers: Optional(ProviderUserInputAnswers),
    attachmentsByQuestionId: Optional(UserInputAttachments),
  }),
  Object({
    type: Const("thread.user-input.dismiss"),
    commandId: CommandId,
    threadId: ThreadId,
    requestId: RuntimeRequestId,
  }),
  Object({
    type: Const("checkpoint.rollback"),
    restoreFiles: Optional(Boolean),
    commandId: CommandId,
    threadId: ThreadId,
    scopeId: CheckpointScopeId,
    checkpointId: CheckpointId,
  }),
  Object({
    type: Const("thread.fork"),
    ...OrchestrationV2CreationFields,
    commandId: CommandId,
    sourceThreadId: ThreadId,
    targetThreadId: ThreadId,
    sourcePoint: OrchestrationV2ThreadForkSourcePoint,
    title: Optional(TrimmedNonEmptyString),
    createdAt: Optional(DateTimeUtc),
  }),
  Object({
    type: Const("thread.merge_back"),
    ...OrchestrationV2CreationFields,
    commandId: CommandId,
    sourceThreadId: ThreadId,
    targetThreadId: ThreadId,
    sourcePoint: OrchestrationV2ThreadForkSourcePoint,
    createdAt: Optional(DateTimeUtc),
  }),
  Object({
    type: Const("delegated_task.request"),
    ...OrchestrationV2CreationFields,
    commandId: CommandId,
    parentThreadId: ThreadId,
    parentRunId: RunId,
    parentNodeId: NodeId,
    task: TrimmedNonEmptyString,
    title: Optional(TrimmedNonEmptyString),
    modelSelection: ModelSelection,
    runtimeMode: RuntimeMode,
    interactionMode: ProviderInteractionMode,
    workspaceStrategy: Optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
    completionWake: Optional(Enum(["always", "settled_only"])),
    createdAt: Optional(DateTimeUtc),
  }),
  Object({
    type: Const("delegated_task.wake-policy"),
    commandId: CommandId,
    parentThreadId: ThreadId,
    taskId: NodeId,
    completionWake: Enum(["always", "settled_only"]),
  }),
  Object({
    type: Const("delegated_task.completion-delivery.acknowledge"),
    commandId: CommandId,
    parentThreadId: ThreadId,
    taskId: NodeId,
    observedByRunId: Nullable(RunId),
  }),
  Object({
    type: Const("delegated_task.completion-delivery.dispose"),
    commandId: CommandId,
    parentThreadId: ThreadId,
    taskId: NodeId,
  }),
  Object({
    type: Const("thread.created.record"),
    commandId: CommandId,
    parentThreadId: ThreadId,
    parentRunId: RunId,
    parentNodeId: NodeId,
    targetThreadId: ThreadId,
    targetRunId: Nullable(RunId),
  }),
  Object({
    type: Const("provider.switch"),
    commandId: CommandId,
    threadId: ThreadId,
    modelSelection: ModelSelection,
  }),
])
```

### OrchestrationV2CommandOutputError

Source: `packages/contracts/src/orchestrationV2.ts:3357`.

```text
Object({_tag: Const("OrchestrationV2CommandOutputError"),
    threadId: ThreadId,
    message: String,
    cause: Optional(Defect()),
  })
```

### OrchestrationV2CommandOutputFrame

Source: `packages/contracts/src/orchestrationV2.ts:3348`.

```text
Object({
  kind: Enum(["replace", "append"]),
  text: String,
  truncated: Boolean,
  running: Boolean,
})
```

### OrchestrationV2ContextCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:302`.

```text
Object({
  acceptsSystemContext: Boolean,
  acceptsDeveloperContext: Boolean,
  acceptsSyntheticUserContext: Boolean,
  canGenerateSummaries: Boolean,
  canConsumeHandoffSummaries: Boolean,
  supportsDeltaHandoff: Boolean,
  supportsFullThreadHandoff: Boolean,
  maxRecommendedHandoffChars: Nullable(PositiveInt),
})
```

### OrchestrationV2ContextHandoff

Source: `packages/contracts/src/orchestrationV2.ts:1030`.

```text
Object({
  id: ContextHandoffId,
  transferId: Optional(Nullable(ContextTransferId)),
  threadId: ThreadId,
  targetRunId: RunId,
  fromProviderThreadIds: Array(ProviderThreadId),
  toProviderThreadId: ProviderThreadId,
  coveredRunOrdinals: Object({
    from: PositiveInt,
    to: PositiveInt,
  }),
  strategy: Enum([
    "delta_since_target_last_seen",
    "fork_delta_summary",
    "full_thread_summary",
    "checkpoint_summary",
    "manual_context",
  ]),
  status: Enum(["pending", "ready", "failed", "superseded"]),
  summaryMessageId: Nullable(MessageId),
  summaryText: String,
  history: Optional(
    Object({
      messages: Array(OrchestrationV2HistoricalMessage),
      coverage: String,
      omittedItems: NonNegativeInt,
      omittedItemIds: Optional(Array(TurnItemId)),
    }),
  delivery: Optional(
    Object({
      nativeThreadId: String,
      status: Enum(["pending", "injected", "inline"]),
      itemIds: Array(TurnItemId),
      omittedItemIds: Optional(Array(TurnItemId)),
    }),
  detailInTurnItem: Optional(Const(true)),
  createdByProviderInstanceId: Nullable(ProviderInstanceId),
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
})
```

### OrchestrationV2ContextHandoffJson

Source: `packages/contracts/src/orchestrationV2.ts:2218`.

```text
MergeFields(OrchestrationV2ContextHandoff, {
    createdAt: DateTimeUtcFromString,
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2ContextSourcePoint

Source: `packages/contracts/src/orchestrationV2.ts:134`.

```text
Object({
  threadId: ThreadId,
  runId: Optional(RunId),
  checkpointId: Optional(CheckpointId),
  turnItemId: Optional(TurnItemId),
  providerThreadRef: Optional(OrchestrationV2ProviderRef),
  providerTurnRef: Optional(OrchestrationV2ProviderRef),
})
```

### OrchestrationV2ContextTransfer

Source: `packages/contracts/src/orchestrationV2.ts:191`.

```text
Object({
  id: ContextTransferId,
  type: OrchestrationV2ContextTransferType,
  sourceThreadId: ThreadId,
  targetThreadId: ThreadId,
  sourcePoint: OrchestrationV2ContextSourcePoint,
  basePoint: Nullable(OrchestrationV2ContextSourcePoint),
  sourceProviderInstanceId: Nullable(ProviderInstanceId),
  sourceTokenUsage: Optional(OrchestrationV2ProviderTurnTokenUsage),
  targetProviderInstanceId: Nullable(ProviderInstanceId),
  targetRunId: Nullable(RunId),
  status: Enum([
    "pending",
    "resolved_native",
    "resolved_portable",
    "failed",
    "consumed",
    "superseded",
  ]),
  resolution: Nullable(OrchestrationV2ContextTransferResolution),
  createdBy: OrchestrationV2Actor,
  error: Nullable(String),
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
  consumedAt: Nullable(DateTimeUtc),
})
```

### OrchestrationV2ContextTransferJson

Source: `packages/contracts/src/orchestrationV2.ts:2227`.

```text
MergeFields(OrchestrationV2ContextTransfer, {
    createdAt: DateTimeUtcFromString,
    updatedAt: DateTimeUtcFromString,
    consumedAt: Nullable(DateTimeUtcFromString),
  })
```

### OrchestrationV2ContextTransferResolution

Source: `packages/contracts/src/orchestrationV2.ts:151`.

```text
Union([
  Object({
    strategy: Const("native_fork"),
    providerThreadRef: OrchestrationV2ProviderRef,
  }),
  Object({
    strategy: Const("portable_context"),
    contextHandoffId: ContextHandoffId,
  }),
  Object({
    strategy: Const("delta_context"),
    contextHandoffId: ContextHandoffId,
  }),
  Object({
    strategy: Const("fork_delta_context"),
    contextHandoffId: ContextHandoffId,
  }),
  Object({
    strategy: Const("checkpoint_context"),
    contextHandoffId: ContextHandoffId,
  }),
])
```

### OrchestrationV2ContextTransferType

Source: `packages/contracts/src/orchestrationV2.ts:125`.

```text
Enum([
  "fork",
  "provider_handoff",
  "merge_back",
  "subagent_spawn",
  "subagent_result",
])
```

### OrchestrationV2ConversationMessage

Source: `packages/contracts/src/orchestrationV2.ts:1194`.

```text
Object({
  notification: Optional(OrchestrationV2Notification),
  ...OrchestrationV2CreationFields,
  scheduledTaskId: Optional(ScheduledTaskId),
  senderThreadId: Optional(ThreadId),
  idleCompaction: Optional(OrchestrationV2IdleCompaction),
  workflowScript: Optional(Boolean),
  id: MessageId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  nodeId: Nullable(NodeId),
  role: Enum(["user", "assistant", "system"]),
  text: String,
  context: Optional(OrchestrationMessageContext),
  attachments: Array(ChatAttachment),
  streaming: Boolean,
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
  delegatedCompletion: Optional(
    Object({
      parentRunId: RunId,
      generation: PositiveInt,
      taskIds: Array(NodeId),
    }),
})
```

### OrchestrationV2ConversationMessageJson

Source: `packages/contracts/src/orchestrationV2.ts:2253`.

```text
MergeFields(OrchestrationV2ConversationMessage, {
    createdAt: DateTimeUtcFromString,
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2CreationFields

Source: `packages/contracts/src/orchestrationV2.ts:101`.

```text
{
  createdBy: OrchestrationV2Actor,
  creationSource: OrchestrationV2CreationSource,
}
```

### OrchestrationV2CreationSource

Source: `packages/contracts/src/orchestrationV2.ts:89`.

```text
Enum([
  "web",
  "mobile",
  "mcp",
  "provider",
  "server",
])
```

### OrchestrationV2DelegatedCompletionCohort

Source: `packages/contracts/src/orchestrationV2.ts:582`.

```text
Object({
  disposition: Enum(["open", "stopped", "disposed"]),
  nextGeneration: PositiveInt,
  delivery: Nullable(OrchestrationV2DelegatedCompletionDelivery),
})
```

### OrchestrationV2DelegatedCompletionDelivery

Source: `packages/contracts/src/orchestrationV2.ts:574`.

```text
Object({
  generation: PositiveInt,
  messageId: MessageId,
  taskIds: Array(NodeId),
})
```

### OrchestrationV2DelegatedCompletionTaskDelivery

Source: `packages/contracts/src/orchestrationV2.ts:567`.

```text
Object({
  state: OrchestrationV2DelegatedCompletionTaskDeliveryState,
  observedByRunId: Nullable(RunId),
})
```

### OrchestrationV2DelegatedCompletionTaskDeliveryState

Source: `packages/contracts/src/orchestrationV2.ts:557`.

```text
Enum([
  "pending",
  "claimed",
  "acknowledged",
  "delivered",
  "disposed",
])
```

### OrchestrationV2DispatchCommandError

Source: `packages/contracts/src/orchestrationV2.ts:3668`.

```text
Object({_tag: Const("OrchestrationV2DispatchCommandError"),
    commandId: CommandId,
    commandType: String,
    message: String,
    detail: Optional(String),
    cause: Optional(Defect()),
  })
```

### OrchestrationV2DispatchCommandResult

Source: `packages/contracts/src/orchestrationV2.ts:3423`.

```text
Object({
  sequence: NonNegativeInt,
})
```

### OrchestrationV2DomainEvent

Source: `packages/contracts/src/orchestrationV2.ts:1720`.

```text
Union([
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("thread.created"),
    payload: OrchestrationV2AppThread,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Enum([
      "thread.archived",
      "thread.unarchived",
      "thread.deleted",
      "thread.settled",
      "thread.unsettled",
      "thread.snoozed",
      "thread.unsnoozed",
      "thread.pinned",
      "thread.auto-settle-set",
      "thread.unpinned",
      "thread.pin-reordered",
      "thread.active-reordered",
      "thread.visited",
      "thread.marked-unread",
      "thread.metadata-updated",
      "thread.pull-request-synced",
      "thread.runtime-mode-updated",
      "thread.interaction-mode-updated",
      "thread.model-selection-updated",
      "thread.provider-switched",
      "thread.guest-share-updated",
    ]),
    payload: OrchestrationV2AppThread,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("thread.pull-request-link-updated"),
    payload: ThreadPullRequestLink,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("run.created"),
    payload: OrchestrationV2Run,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("run.updated"),
    payload: OrchestrationV2Run,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("run.background-work-cancelled"),
    payload: OrchestrationV2RunBackgroundWorkCancelled,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("run-attempt.created"),
    payload: OrchestrationV2RunAttempt,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("run-attempt.updated"),
    payload: OrchestrationV2RunAttempt,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("node.updated"),
    payload: OrchestrationV2ExecutionNode,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("subagent.updated"),
    payload: OrchestrationV2Subagent,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Enum(["provider-session.attached", "provider-session.updated"]),
    payload: OrchestrationV2ProviderSession,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("provider-session.detached"),
    payload: OrchestrationV2ProviderSessionDetached,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("provider-thread.updated"),
    payload: OrchestrationV2ProviderThread,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("provider-turn.updated"),
    payload: OrchestrationV2ProviderTurn,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("runtime-request.updated"),
    payload: OrchestrationV2RuntimeRequest,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("message.updated"),
    payload: OrchestrationV2ConversationMessage,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("turn-item.updated"),
    payload: OrchestrationV2TurnItem,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("plan.updated"),
    payload: OrchestrationV2PlanArtifact,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("checkpoint-scope.created"),
    payload: OrchestrationV2CheckpointScope,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("checkpoint.captured"),
    payload: OrchestrationV2Checkpoint,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("checkpoint.rollback-requested"),
    payload: OrchestrationV2CheckpointRollbackRequest,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("context-handoff.updated"),
    payload: OrchestrationV2ContextHandoff,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("context-transfer.created"),
    payload: OrchestrationV2ContextTransfer,
  }),
  Object({
    ...OrchestrationV2EventBase.fields,
    type: Const("context-transfer.updated"),
    payload: OrchestrationV2ContextTransfer,
  }),
])
```

### OrchestrationV2DomainEventJson

Source: `packages/contracts/src/orchestrationV2.ts:2600`.

```text
Union([
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("thread.created"),
    payload: OrchestrationV2AppThreadJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Enum([
      "thread.archived",
      "thread.unarchived",
      "thread.deleted",
      "thread.settled",
      "thread.unsettled",
      "thread.snoozed",
      "thread.unsnoozed",
      "thread.pinned",
      "thread.auto-settle-set",
      "thread.unpinned",
      "thread.pin-reordered",
      "thread.active-reordered",
      "thread.visited",
      "thread.marked-unread",
      "thread.pull-request-synced",
      "thread.metadata-updated",
      "thread.runtime-mode-updated",
      "thread.interaction-mode-updated",
      "thread.model-selection-updated",
      "thread.provider-switched",
      "thread.guest-share-updated",
    ]),
    payload: OrchestrationV2AppThreadJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("thread.pull-request-link-updated"),
    payload: ThreadPullRequestLink,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("run.created"),
    payload: OrchestrationV2RunJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("run.updated"),
    payload: OrchestrationV2RunJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("run.background-work-cancelled"),
    payload: OrchestrationV2RunBackgroundWorkCancelled,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("run-attempt.created"),
    payload: OrchestrationV2RunAttemptJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("run-attempt.updated"),
    payload: OrchestrationV2RunAttemptJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("node.updated"),
    payload: OrchestrationV2ExecutionNodeJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("subagent.updated"),
    payload: OrchestrationV2SubagentJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Enum(["provider-session.attached", "provider-session.updated"]),
    payload: OrchestrationV2ProviderSessionJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("provider-session.detached"),
    payload: OrchestrationV2ProviderSessionDetachedJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("provider-thread.updated"),
    payload: OrchestrationV2ProviderThreadJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("provider-turn.updated"),
    payload: OrchestrationV2ProviderTurnJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("runtime-request.updated"),
    payload: OrchestrationV2RuntimeRequestJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("message.updated"),
    payload: OrchestrationV2ConversationMessageJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("turn-item.updated"),
    payload: OrchestrationV2TurnItemJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("plan.updated"),
    payload: OrchestrationV2PlanArtifact,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("checkpoint-scope.created"),
    payload: OrchestrationV2CheckpointScopeJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("checkpoint.captured"),
    payload: OrchestrationV2CheckpointJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("checkpoint.rollback-requested"),
    payload: OrchestrationV2CheckpointRollbackRequestJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("context-handoff.updated"),
    payload: OrchestrationV2ContextHandoffJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("context-transfer.created"),
    payload: OrchestrationV2ContextTransferJson,
  }),
  Object({
    ...OrchestrationV2JsonEventBaseFields,
    type: Const("context-transfer.updated"),
    payload: OrchestrationV2ContextTransferJson,
  }),
])
```

### OrchestrationV2EventBase

Source: `packages/contracts/src/orchestrationV2.ts:1709`.

```text
Object({
  id: EventId,
  threadId: ThreadId,
  runId: Optional(RunId),
  nodeId: Optional(NodeId),
  driver: Optional(ProviderDriverKind),
  providerInstanceId: Optional(ProviderInstanceId),
  rawEventId: Optional(RawEventId),
  occurredAt: DateTimeUtc,
})
```

### OrchestrationV2ExecutionNode

Source: `packages/contracts/src/orchestrationV2.ts:695`.

```text
Object({
  id: NodeId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  parentNodeId: Nullable(NodeId),
  rootNodeId: NodeId,
  kind: Enum([
    "root_turn",
    "assistant_message",
    "reasoning",
    "plan",
    "todo_list",
    "tool_call",
    "approval_request",
    "user_input_request",
    "subagent",
    "hook",
    "system",
  ]),
  status: Enum([
    "idle",
    "pending",
    "running",
    "waiting",
    "completed",
    "interrupted",
    "failed",
    "cancelled",
    "rolled_back",
  ]),
  countsForRun: Boolean,
  providerThreadId: Nullable(ProviderThreadId),
  providerTurnId: Nullable(ProviderTurnId),
  nativeItemRef: Nullable(OrchestrationV2ProviderRef),
  runtimeRequestId: Nullable(RuntimeRequestId),
  checkpointScopeId: Nullable(CheckpointScopeId),
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
})
```

### OrchestrationV2ExecutionNodeJson

Source: `packages/contracts/src/orchestrationV2.ts:2167`.

```text
MergeFields(OrchestrationV2ExecutionNode, {
    startedAt: Nullable(DateTimeUtcFromString),
    completedAt: Nullable(DateTimeUtcFromString),
  })
```

### OrchestrationV2FileChangeDetail

Source: `packages/contracts/src/orchestrationV2.ts:1335`.

```text
Object({
  operation: TrimmedNonEmptyString,
  path: TrimmedNonEmptyString,
  oldPath: Optional(TrimmedNonEmptyString),
  fileType: Optional(TrimmedNonEmptyString),
  mimeType: Optional(TrimmedNonEmptyString),
})
```

### OrchestrationV2FileSearchResult

Source: `packages/contracts/src/orchestrationV2.ts:1410`.

```text
Object({
  fileName: TrimmedNonEmptyString,
  line: Optional(PositiveInt),
  column: Optional(PositiveInt),
  preview: Optional(String),
})
```

### OrchestrationV2GetOlderSettledThreadsInput

Source: `packages/contracts/src/orchestrationV2.ts:3503`.

```text
Object({
  before: OrchestrationV2OlderSettledCursor,
  projectIds: Optional(Array(ProjectId)),
  limit: Int.check(isBetween({ minimum: 0, maximum: 50 }),
})
```

### OrchestrationV2GetShellSnapshotError

Source: `packages/contracts/src/orchestrationV2.ts:3688`.

```text
Object({_tag: Const("OrchestrationV2GetShellSnapshotError"),
    message: String,
    cause: Optional(Defect()),
  })
```

### OrchestrationV2GetThreadFamilyInput

Source: `packages/contracts/src/orchestrationV2.ts:3450`.

```text
Object({
  threadId: ThreadId,
})
```

### OrchestrationV2GetThreadProjectionError

Source: `packages/contracts/src/orchestrationV2.ts:3679`.

```text
Object({_tag: Const("OrchestrationV2GetThreadProjectionError"),
    threadId: ThreadId,
    message: String,
    cause: Optional(Defect()),
  })
```

### OrchestrationV2GetThreadProjectionInput

Source: `packages/contracts/src/orchestrationV2.ts:3428`.

```text
Object({
  threadId: ThreadId,
})
```

### OrchestrationV2GetTurnItemInput

Source: `packages/contracts/src/orchestrationV2.ts:3792`.

```text
Object({
  threadId: ThreadId,
  itemId: TurnItemId,
  revision: Optional(String),
})
```

### OrchestrationV2GetTurnItemResult

Source: `packages/contracts/src/orchestrationV2.ts:3799`.

```text
Object({
  item: Nullable(OrchestrationV2TurnItem),
})
```

### OrchestrationV2GetWorkflowScriptInput

Source: `packages/contracts/src/orchestrationV2.ts:3744`.

```text
Object({
  threadId: ThreadId,
  scriptPath: TrimmedNonEmptyString,
})
```

### OrchestrationV2GetWorkflowScriptResult

Source: `packages/contracts/src/orchestrationV2.ts:3751`.

```text
Object({
  scriptPath: TrimmedNonEmptyString,
  contents: String,
  truncated: Boolean,
})
```

### OrchestrationV2GetWorktreeThreadIdsInput

Source: `packages/contracts/src/orchestrationV2.ts:3466`.

```text
Object({
  worktreePath: TrimmedNonEmptyString,
})
```

### OrchestrationV2HistoricalMessage

Source: `packages/contracts/src/orchestrationV2.ts:1017`.

```text
Object({
  role: Enum(["user", "assistant"]),
  text: String,
  runStatus: Optional(String),
  threadId: ThreadId,
  runId: Nullable(RunId),
  itemId: TurnItemId,
  providerThreadId: Nullable(ProviderThreadId),
  status: String,
  kind: String,
})
```

### OrchestrationV2IdentityCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:330`.

```text
Object({
  nativeThreadIds: OrchestrationV2NativeRefStrength,
  nativeTurnIds: OrchestrationV2NativeRefStrength,
  nativeItemIds: OrchestrationV2NativeRefStrength,
  nativeRequestIds: OrchestrationV2NativeRefStrength,
})
```

### OrchestrationV2IdleCompaction

Source: `packages/contracts/src/orchestrationV2.ts:1191`.

```text
Enum(["beforeCacheExpires", "onReturn"])
```

### OrchestrationV2InternalCommand

Source: `packages/contracts/src/orchestrationV2.ts:3220`.

```text
Union([
  Object({
    type: Const("thread.guest-share.open"),
    commandId: CommandId,
    threadId: ThreadId,
    shareId: GuestShareId,
    host: TrimmedNonEmptyString,
    expiresAt: IsoDateTime,
  }),
  Object({
    type: Const("thread.guest-share.redeem"),
    commandId: CommandId,
    threadId: ThreadId,
    shareId: GuestShareId,
    guestName: TrimmedNonEmptyString,
  }),
  Object({
    type: Const("thread.guest-message.submit"),
    commandId: CommandId,
    threadId: ThreadId,
    shareId: GuestShareId,
    guestMessageId: GuestMessageId,
    text: String,
    attachments: Optional(Array(ChatAttachment)),
  }),
  Object({
    type: Const("thread.pull-request-watch.sync"),
    commandId: CommandId,
    threadId: ThreadId,
    ...ThreadPullRequestKey.fields,
    startedAt: IsoDateTime,
    watch: Nullable(ThreadPullRequestWatch),
    endedWatch: Optional(ThreadPullRequestWatch),
    wake: Optional(
      Object({
        messageId: MessageId,
        text: String,
        notification: OrchestrationV2Notification,
      }),
  }),
  Object({
    type: Const("checkpoint.rollback.fail"),
    commandId: CommandId,
    threadId: ThreadId,
    requestId: CommandId,
    message: TrimmedNonEmptyString,
  }),
  Object({
    type: Const("runtime-request.delivery.fail"),
    commandId: CommandId,
    threadId: ThreadId,
    requestId: RuntimeRequestId,
  }),
  Object({
    type: Const("thread.background-work.settle"),
    commandId: CommandId,
    threadId: ThreadId,
    providerThreadId: ProviderThreadId,
    providerTurnId: ProviderTurnId,
    subagent: Optional(
      Object({
        id: NodeId,
        stoppedAt: DateTimeUtc,
        outcome: Object({
          status: Enum(["completed", "failed", "cancelled"]),
          completedAt: DateTimeUtc,
          result: Nullable(String),
        }),
      }),
  }),
  Object({
    type: Const("secret_request.record"),
    commandId: CommandId,
    threadId: ThreadId,
    runId: RunId,
    nodeId: NodeId,
    turnItemId: TurnItemId,
    label: TrimmedNonEmptyString,
    reason: String,
    placeholder: Optional(String),
    delivery: Optional(OrchestrationV2SecretRequestDelivery),
    secretStatus: OrchestrationV2SecretRequestStatus,
  }),
])
```

### OrchestrationV2JsonEventBaseFields

Source: `packages/contracts/src/orchestrationV2.ts:2587`.

```text
{
  ...OrchestrationV2EventBase.fields,
  occurredAt: DateTimeUtcFromString,
}
```

### OrchestrationV2LatestVisibleMessageSummary

Source: `packages/contracts/src/orchestrationV2.ts:1918`.

```text
Object({
  id: MessageId,
  role: OrchestrationV2ConversationMessage.fields.role,
  text: String,
  updatedAt: DateTimeUtc,
})
```

### OrchestrationV2LatestVisibleMessageSummaryJson

Source: `packages/contracts/src/orchestrationV2.ts:2538`.

```text
MergeFields(OrchestrationV2LatestVisibleMessageSummary, {
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2LimitRecovery

Source: `packages/contracts/src/orchestrationV2.ts:357`.

```text
Object({
  requestId: Optional(CommandId),
  runId: RunId,
  resetAt: IsoDateTime,
  autoResume: Boolean,
  snooze: Optional(Boolean),
})
```

### OrchestrationV2LimitRecoveryUpdate

Source: `packages/contracts/src/orchestrationV2.ts:366`.

```text
Object({
  runId: RunId,
  resetAt: IsoDateTime,
  autoResume: Optional(Boolean),
  snooze: Optional(Boolean),
}).check(
  Predicate('A recovery update must include autoResume or snooze.'),
)
```

### OrchestrationV2NativeRefStrength

Source: `packages/contracts/src/orchestrationV2.ts:106`.

```text
Enum(["strong", "weak", "none"])
```

### OrchestrationV2Notification

Source: `packages/contracts/src/orchestrationV2.ts:1182`.

```text
Object({
  source: OrchestrationV2NotificationSource,
  outcome: Enum(["completed", "failed", "cancelled", "updated", "unknown"]),
  summary: TrimmedNonEmptyString,
  detail: Optional(String),
})
```

### OrchestrationV2NotificationSource

Source: `packages/contracts/src/orchestrationV2.ts:1145`.

```text
Union([
    Object({
      kind: Const("delegated_task"),
      taskIds: Array(NodeId),
      childThreadId: Optional(ThreadId),
    }),
    Object({
      kind: Const("background_task"),
      work: Const("subagent"),
      childThreadId: Optional(ThreadId),
    }),
    Object({ kind: Const("background_command").transform("command") }),
    SubagentNotificationSource,
    CommandNotificationSource,
    Object({ kind: Const("monitor") }),
    Object({ kind: Const("background_task") }),
    Object({ kind: Const("system") }),
  ])
```

### OrchestrationV2OlderSettledCount

Source: `packages/contracts/src/orchestrationV2.ts:2048`.

```text
Object({
  projectId: ProjectId,
  count: NonNegativeInt,
})
```

### OrchestrationV2OlderSettledCursor

Source: `packages/contracts/src/orchestrationV2.ts:3497`.

```text
Object({
  settledAt: IsoDateTime,
  threadId: Optional(ThreadId),
})
```

### OrchestrationV2OlderSettledThreads

Source: `packages/contracts/src/orchestrationV2.ts:2059`.

```text
Object({
  before: IsoDateTime,
  asOfSequence: NonNegativeInt,
  counts: Array(OrchestrationV2OlderSettledCount),
})
```

### OrchestrationV2OlderSettledThreadsPage

Source: `packages/contracts/src/orchestrationV2.ts:3518`.

```text
Object({
  snapshotSequence: NonNegativeInt,
  threads: Array(OrchestrationV2ThreadShell),
  counts: Array(OrchestrationV2OlderSettledCount),
})
```

### OrchestrationV2PendingBackgroundTask

Source: `packages/contracts/src/orchestrationV2.ts:923`.

```text
Union([
    Object({
      ...PendingBackgroundTaskFields,
      kind: Const("subagent"),
      providerInstanceId: Optional(ProviderInstanceId),
      childThreadId: Optional(ThreadId),
    }),
    Object({ ...PendingBackgroundTaskFields, kind: Const("command") }),
    Object({ ...PendingBackgroundTaskFields, kind: Const("monitor") }),
    Object({ ...PendingBackgroundTaskFields, kind: Const("background_task") }),
  ])
```

### OrchestrationV2PendingRuntimeRequestSummary

Source: `packages/contracts/src/orchestrationV2.ts:1908`.

```text
Object({
  id: RuntimeRequestId,
  kind: OrchestrationV2RuntimeRequest.fields.kind,
  createdAt: DateTimeUtc,
  blocking: Optional(Boolean),
})
```

### OrchestrationV2PendingRuntimeRequestSummaryJson

Source: `packages/contracts/src/orchestrationV2.ts:2530`.

```text
MergeFields(OrchestrationV2PendingRuntimeRequestSummary, {
    createdAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2PlanArtifact

Source: `packages/contracts/src/orchestrationV2.ts:1263`.

```text
Union([
  Object({
    ...OrchestrationV2PlanArtifactBaseFields,
    kind: Const("proposed_plan"),
    markdown: String,
  }),
  Object({
    ...OrchestrationV2PlanArtifactBaseFields,
    kind: Const("todo_list"),
    steps: Array(OrchestrationV2PlanStep),
    explanation: Optional(String),
  }),
])
```

### OrchestrationV2PlanArtifactBaseFields

Source: `packages/contracts/src/orchestrationV2.ts:1254`.

```text
{
  id: PlanId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  nodeId: NodeId,
  status: Enum(["draft", "active", "completed", "superseded"]),
  detailInTurnItem: Optional(Const(true)),
}
```

### OrchestrationV2PlanStep

Source: `packages/contracts/src/orchestrationV2.ts:1226`.

```text
Object({
  id: TrimmedNonEmptyString,
  text: TrimmedNonEmptyString,
  status: Enum(["pending", "running", "completed"]),
  durationAnchorAt: Optional(IsoDateTime),
  durationMs: Optional(NonNegativeInt),
})
```

### OrchestrationV2PlanningCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:283`.

```text
Object({
  emitsPlanUpdated: Boolean,
  emitsTodoList: Boolean,
  emitsProposedPlan: Boolean,
  supportsStructuredQuestions: Boolean,
  planDeltasHaveItemIds: Boolean,
})
```

### OrchestrationV2ProjectedTurnItem

Source: `packages/contracts/src/orchestrationV2.ts:1685`.

```text
Object({
  position: NonNegativeInt,
  visibility: Enum(["local", "inherited", "synthetic"]),
  sourceThreadId: ThreadId,
  sourceItemId: TurnItemId,
  item: OrchestrationV2TurnItem,
})
```

### OrchestrationV2ProviderCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:338`.

```text
Object({
  sessions: OrchestrationV2SessionCapabilities,
  threads: OrchestrationV2ThreadCapabilities,
  turns: OrchestrationV2TurnCapabilities,
  streaming: OrchestrationV2StreamingCapabilities,
  tools: OrchestrationV2ToolCapabilities,
  approvals: OrchestrationV2ApprovalCapabilities,
  planning: OrchestrationV2PlanningCapabilities,
  subagents: OrchestrationV2SubagentCapabilities,
  context: OrchestrationV2ContextCapabilities,
  checkpointing: OrchestrationV2CheckpointCapabilities,
  identity: OrchestrationV2IdentityCapabilities,
  runtimePolicy: OrchestrationV2RuntimePolicyCapabilities,
})
```

### OrchestrationV2ProviderFailure

Source: `packages/contracts/src/orchestrationV2.ts:1360`.

```text
Object({
  class: OrchestrationV2ProviderFailureClass,
  message: OrchestrationV2ProviderFailureMessage,
  code: Nullable(OrchestrationV2ProviderFailureCode),
  retryable: Nullable(Boolean),
  resetAt: Optional(Nullable(IsoDateTime)),
})
```

### OrchestrationV2ProviderFailureClass

Source: `packages/contracts/src/orchestrationV2.ts:1344`.

```text
Enum([
  "usage_limit",
  "provider_error",
  "transport_error",
  "permission_error",
  "validation_error",
  "unknown",
])
```

### OrchestrationV2ProviderFailureCode

Source: `packages/contracts/src/orchestrationV2.ts:1357`.

```text
TrimmedNonEmptyString.check(isMaxLength(128))
```

### OrchestrationV2ProviderFailureMessage

Source: `packages/contracts/src/orchestrationV2.ts:1354`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(4_096),
)
```

### OrchestrationV2ProviderGoal

Source: `packages/contracts/src/orchestrationV2.ts:969`.

```text
Object({
  objective: TrimmedNonEmptyString,
  status: OrchestrationV2ProviderGoalStatus,
  tokensUsed: Optional(NonNegativeInt),
  tokenBudget: Optional(Nullable(NonNegativeInt)),
  timeUsedSeconds: Optional(NonNegativeInt),
  checks: Optional(NonNegativeInt),
  lastCheck: Optional(String),
})
```

### OrchestrationV2ProviderGoalStatus

Source: `packages/contracts/src/orchestrationV2.ts:959`.

```text
Enum([
  "active",
  "paused",
  "blocked",
  "usage_limited",
  "budget_limited",
  "complete",
])
```

### OrchestrationV2ProviderRef

Source: `packages/contracts/src/orchestrationV2.ts:109`.

```text
Object({
  driver: ProviderDriverKind,
  nativeId: Nullable(TrimmedNonEmptyString),
  strength: OrchestrationV2NativeRefStrength,
  fingerprint: Optional(TrimmedNonEmptyString),
  ordinal: Optional(NonNegativeInt),
})
```

### OrchestrationV2ProviderRetry

Source: `packages/contracts/src/orchestrationV2.ts:1369`.

```text
Object({
  attempt: PositiveInt,
  maxAttempts: Nullable(PositiveInt),
  retryDelayMs: Nullable(NonNegativeInt),
})
```

### OrchestrationV2ProviderSession

Source: `packages/contracts/src/orchestrationV2.ts:852`.

```text
Object({
  id: ProviderSessionId,
  driver: ProviderDriverKind,
  providerInstanceId: ProviderInstanceId,
  status: Enum(["starting", "ready", "running", "waiting", "stopped", "error"]),
  cwd: TrimmedNonEmptyString,
  model: Nullable(TrimmedNonEmptyString),
  capabilities: OrchestrationV2ProviderCapabilities,
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
  lastError: Nullable(String),
})
```

### OrchestrationV2ProviderSessionDetached

Source: `packages/contracts/src/orchestrationV2.ts:866`.

```text
Object({
  providerSessionId: ProviderSessionId,
  detachedAt: DateTimeUtc,
  reason: Optional(String),
})
```

### OrchestrationV2ProviderSessionDetachedJson

Source: `packages/contracts/src/orchestrationV2.ts:2201`.

```text
MergeFields(OrchestrationV2ProviderSessionDetached, {
    detachedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2ProviderSessionJson

Source: `packages/contracts/src/orchestrationV2.ts:2192`.

```text
MergeFields(OrchestrationV2ProviderSession, {
    createdAt: DateTimeUtcFromString,
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2ProviderThread

Source: `packages/contracts/src/orchestrationV2.ts:980`.

```text
Object({
  id: ProviderThreadId,
  driver: ProviderDriverKind,
  providerInstanceId: ProviderInstanceId,
  providerSessionId: Nullable(ProviderSessionId),
  appThreadId: Nullable(ThreadId),
  ownerNodeId: Nullable(NodeId),
  nativeThreadRef: Nullable(OrchestrationV2ProviderRef),
  nativeConversationHeadRef: Nullable(OrchestrationV2ProviderRef),
  status: Enum(["not_loaded", "idle", "active", "archived", "closed", "error"]),
  firstRunOrdinal: Nullable(PositiveInt),
  lastRunOrdinal: Nullable(PositiveInt),
  handoffIds: Array(ContextHandoffId),
  forkedFrom: Nullable(
    Object({
      providerThreadId: ProviderThreadId,
      providerTurnId: Optional(ProviderTurnId),
      checkpointId: Optional(CheckpointId),
    }),
  pendingBackgroundTasks: Optional(Array(OrchestrationV2PendingBackgroundTask)),
  contextUsage: Optional(Nullable(ThreadTokenUsageSnapshot)),
  nativeMetadata: Optional(Nullable(OrchestrationV2ProviderThreadNativeMetadata)),
  goal: Optional(Nullable(OrchestrationV2ProviderGoal)),
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
})
```

### OrchestrationV2ProviderThreadDisposition

Source: `packages/contracts/src/orchestrationV2.ts:1376`.

```text
Enum(["reusable", "broken"])
```

### OrchestrationV2ProviderThreadJson

Source: `packages/contracts/src/orchestrationV2.ts:2209`.

```text
MergeFields(OrchestrationV2ProviderThread, {
    createdAt: DateTimeUtcFromString,
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2ProviderThreadNativeMetadata

Source: `packages/contracts/src/orchestrationV2.ts:946`.

```text
Object({
  modelSelection: Optional(ModelSelection),
  title: Optional(Nullable(TrimmedNonEmptyString)),
  updatedAt: Optional(Nullable(TrimmedNonEmptyString)),
  itemIdentityVersion: Optional(Const(2)),
  forkHeadMessageId: Optional(TrimmedNonEmptyString),
  claudeAttachmentBudget: Optional(
    Object({ bytesUsed: NonNegativeInt, pagesUsed: NonNegativeInt }),
})
```

### OrchestrationV2ProviderTurn

Source: `packages/contracts/src/orchestrationV2.ts:1074`.

```text
Object({
  id: ProviderTurnId,
  providerThreadId: ProviderThreadId,
  nodeId: NodeId,
  runAttemptId: Nullable(RunAttemptId),
  nativeTurnRef: Nullable(OrchestrationV2ProviderRef),
  ordinal: PositiveInt,
  status: Enum([
    "pending",
    "running",
    "completed",
    "interrupted",
    "failed",
    "cancelled",
  ]),
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
  tokenUsage: Optional(OrchestrationV2ProviderTurnTokenUsage),
  turnTokenUsage: Optional(TurnTokenUsage),
})
```

### OrchestrationV2ProviderTurnJson

Source: `packages/contracts/src/orchestrationV2.ts:2237`.

```text
MergeFields(OrchestrationV2ProviderTurn, {
  startedAt: Nullable(DateTimeUtcFromString),
  completedAt: Nullable(DateTimeUtcFromString),
})
```

### OrchestrationV2ProviderTurnTokenUsage

Source: `packages/contracts/src/orchestrationV2.ts:176`.

```text
Object({
  usedTokens: NonNegativeInt,
  maxTokens: Optional(Nullable(NonNegativeInt)),
  inputTokens: Optional(NonNegativeInt),
  cachedInputTokens: Optional(NonNegativeInt),
  outputTokens: Optional(NonNegativeInt),
  reasoningOutputTokens: Optional(NonNegativeInt),
  promptCacheTtlMs: Optional(PositiveInt),
  autoCompactThreshold: Optional(PositiveInt),
  promptCacheExpiresAt: Optional(String),
  updatedAt: String,
})
```

### OrchestrationV2RawProviderEvent

Source: `packages/contracts/src/orchestrationV2.ts:1694`.

```text
Object({
  id: RawEventId,
  driver: ProviderDriverKind,
  providerInstanceId: ProviderInstanceId,
  providerSessionId: ProviderSessionId,
  sequence: PositiveInt,
  direction: Enum(["incoming", "outgoing"]),
  messageKind: Enum(["request", "response", "notification", "error"]),
  method: Nullable(TrimmedNonEmptyString),
  jsonRpcId: Nullable(Union([String, Number])),
  payload: Unknown,
  observedAt: DateTimeUtc,
})
```

### OrchestrationV2RawProviderEventJson

Source: `packages/contracts/src/orchestrationV2.ts:2592`.

```text
MergeFields(OrchestrationV2RawProviderEvent, {
    observedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2RestartCancelledBackgroundWork

Source: `packages/contracts/src/orchestrationV2.ts:590`.

```text
Object({
  kind: Enum(["subagent", "shell", "monitor", "task"]),
  label: TrimmedNonEmptyString,
  id: Optional(TrimmedNonEmptyString),
  awaited: Optional(Const(true)),
})
```

### OrchestrationV2RpcError

Source: `packages/contracts/src/orchestrationV2.ts:3735`.

```text
Union([
  OrchestrationV2DispatchCommandError,
  OrchestrationV2GetThreadProjectionError,
  OrchestrationV2GetShellSnapshotError,
  OrchestrationV2ThreadLaunchError,
  OrchestrationV2StopWorkflowError,
])
```

### OrchestrationV2RpcSchemas

Source: `packages/contracts/src/orchestrationV2.ts:3889`.

```text
{
  searchThread: {
    input: OrchestrationV2SearchThreadInput,
    output: OrchestrationV2SearchThreadResult,
  },
  dispatchCommand: {
    input: OrchestrationV2Command,
    output: OrchestrationV2DispatchCommandResult,
  },
  getTurnDiff: {
    input: OrchestrationGetTurnDiffInput,
    output: OrchestrationGetTurnDiffResult,
  },
  getFullThreadDiff: {
    input: OrchestrationGetFullThreadDiffInput,
    output: OrchestrationGetFullThreadDiffResult,
  },
  getArchivedShellSnapshot: {
    input: Object({}),
    output: OrchestrationV2ArchivedShellSnapshot,
  },
  getThreadFamily: {
    input: OrchestrationV2GetThreadFamilyInput,
    output: OrchestrationV2ThreadFamily,
  },
  getWorktreeThreadIds: {
    input: OrchestrationV2GetWorktreeThreadIdsInput,
    output: OrchestrationV2WorktreeThreadIds,
  },
  getOlderSettledThreads: {
    input: OrchestrationV2GetOlderSettledThreadsInput,
    output: OrchestrationV2OlderSettledThreadsPage,
  },
  getThreadProjection: {
    input: OrchestrationV2GetThreadProjectionInput,
    output: OrchestrationV2ThreadProjection,
  },
  stopWorkflow: {
    input: OrchestrationV2StopWorkflowInput,
    output: Void,
  },
  getWorkflowScript: {
    input: OrchestrationV2GetWorkflowScriptInput,
    output: OrchestrationV2GetWorkflowScriptResult,
  },
  getTurnItem: {
    input: OrchestrationV2GetTurnItemInput,
    output: OrchestrationV2GetTurnItemResult,
  },
  launchThread: {
    input: OrchestrationV2ThreadLaunchInput,
    output: OrchestrationV2ThreadLaunchResult,
  },
  subscribeArchivedShell: {
    input: Object({}),
    output: OrchestrationV2ArchivedShellStreamItem,
  },
  subscribeShell: {
    input: OrchestrationV2SubscribeShellInput,
    output: OrchestrationV2ShellStreamItem,
  },
  subscribeThread: {
    input: OrchestrationV2SubscribeThreadInput,
    output: OrchestrationV2ThreadStreamItem,
  },
  subscribeCommandOutput: {
    input: OrchestrationV2SubscribeCommandOutputInput,
    output: OrchestrationV2CommandOutputFrame,
  },
}
```

### OrchestrationV2Run

Source: `packages/contracts/src/orchestrationV2.ts:629`.

```text
Object({
  id: RunId,
  threadId: ThreadId,
  ordinal: PositiveInt,
  providerInstanceId: ProviderInstanceId,
  modelSelection: ModelSelection,
  providerThreadId: Nullable(ProviderThreadId),
  userMessageId: MessageId,
  rootNodeId: Nullable(NodeId),
  activeAttemptId: Nullable(RunAttemptId),
  status: OrchestrationV2RunStatus,
  queuePosition: Optional(Nullable(PositiveInt)),
  queueHeld: Optional(Boolean),
  requestedAt: DateTimeUtc,
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
  checkpointId: Nullable(CheckpointId),
  contextHandoffId: Nullable(ContextHandoffId),
  restartContinuationOfRunId: Optional(RunId),
  delegatedTaskId: Optional(NodeId),
  workStartedAt: Optional(DateTimeUtc),
  restartCancelledBackgroundWork: Optional(
    Array(OrchestrationV2RestartCancelledBackgroundWork),
  ),
  sourcePlanRef: Optional(
    Object({
      threadId: ThreadId,
      planId: PlanId,
    }),
  delegatedCompletion: Optional(OrchestrationV2DelegatedCompletionCohort),
  workspacePreparation: Optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
})
```

### OrchestrationV2RunAttempt

Source: `packages/contracts/src/orchestrationV2.ts:670`.

```text
Object({
  id: RunAttemptId,
  nativeThreadId: Optional(String),
  runId: RunId,
  attemptOrdinal: PositiveInt,
  rootNodeId: NodeId,
  providerInstanceId: ProviderInstanceId,
  providerThreadId: ProviderThreadId,
  providerTurnId: Nullable(ProviderTurnId),
  reason: Enum(["initial", "steering_restart", "retry", "provider_recovery"]),
  status: Enum([
    "pending",
    "running",
    "completed",
    "interrupted",
    "failed",
    "cancelled",
    "superseded",
  ]),
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
})
```

### OrchestrationV2RunAttemptJson

Source: `packages/contracts/src/orchestrationV2.ts:2160`.

```text
MergeFields(OrchestrationV2RunAttempt, {
  startedAt: Nullable(DateTimeUtcFromString),
  completedAt: Nullable(DateTimeUtcFromString),
})
```

### OrchestrationV2RunBackgroundWorkCancelled

Source: `packages/contracts/src/orchestrationV2.ts:600`.

```text
Object({
  runId: RunId,
  restartCancelledBackgroundWork: Array(OrchestrationV2RestartCancelledBackgroundWork),
})
```

### OrchestrationV2RunJson

Source: `packages/contracts/src/orchestrationV2.ts:2151`.

```text
MergeFields(OrchestrationV2Run, {
  requestedAt: DateTimeUtcFromString,
  startedAt: Nullable(DateTimeUtcFromString),
  completedAt: Nullable(DateTimeUtcFromString),
  workStartedAt: Optional(DateTimeUtcFromString),
})
```

### OrchestrationV2RunStatus

Source: `packages/contracts/src/orchestrationV2.ts:543`.

```text
Enum([
  "preparing",
  "queued",
  "starting",
  "running",
  "waiting",
  "completed",
  "interrupted",
  "failed",
  "cancelled",
  "rolled_back",
])
```

### OrchestrationV2RuntimePolicyCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:324`.

```text
Object({
  enforcement: Enum(["native", "client-boundary"]),
})
```

### OrchestrationV2RuntimeRequest

Source: `packages/contracts/src/orchestrationV2.ts:1111`.

```text
Object({
  id: RuntimeRequestId,
  nodeId: NodeId,
  providerTurnId: Nullable(ProviderTurnId),
  nativeRequestRef: Nullable(OrchestrationV2ProviderRef),
  kind: Union([
    ProviderRequestKind,
    Enum(["dynamic_tool_call", "user_input", "auth_refresh"]),
  ]),
  status: Enum(["pending", "resolved", "expired", "cancelled"]),
  responseCapability: Union([
    Object({
      type: Const("live"),
      providerSessionId: ProviderSessionId,
      supportsFeedback: Optional(Boolean),
    }),
    Object({ type: Const("message") }),
    Object({ type: Const("not_resumable"), reason: String }),
  ]),
  createdAt: DateTimeUtc,
  resolvedAt: Nullable(DateTimeUtc),
  decision: Optional(ProviderApprovalDecision),
  feedback: Optional(String),
  answers: Optional(ProviderUserInputAnswers),
})
```

### OrchestrationV2RuntimeRequestJson

Source: `packages/contracts/src/orchestrationV2.ts:2244`.

```text
MergeFields(OrchestrationV2RuntimeRequest, {
    createdAt: DateTimeUtcFromString,
    resolvedAt: Nullable(DateTimeUtcFromString),
  })
```

### OrchestrationV2SearchThreadError

Source: `packages/contracts/src/orchestrationV2.ts:3880`.

```text
Object({_tag: Const("OrchestrationV2SearchThreadError"), cause: Defect() })
```

### OrchestrationV2SearchThreadInput

Source: `packages/contracts/src/orchestrationV2.ts:3837`.

```text
Object({
  threadId: ThreadId,
  skills: Optional(
    Array(
      Object({
        name: String.check(isMaxLength(200)),
        displayName: Optional(String.check(isMaxLength(200))),
      }).check(isMaxLength(1_000)),
  ),
  query: TrimmedNonEmptyString.check(isMaxLength(200)),
  index: Optional(NonNegativeInt),
  offset: Optional(Int),
  start: Optional(
    Object({ entryId: TrimmedNonEmptyString, occurrence: NonNegativeInt }),
})
```

### OrchestrationV2SearchThreadResult

Source: `packages/contracts/src/orchestrationV2.ts:3861`.

```text
Object({
  complete: Optional(Boolean),
  snapshotSequence: NonNegativeInt,
  totalMatches: NonNegativeInt,
  activeIndex: NonNegativeInt,
  match: Nullable(OrchestrationV2ThreadFindMatch),
  navigation: Optional(
    Array(
      Object({
        entryId: TrimmedNonEmptyString,
        runId: Nullable(RunId),
        startIndex: NonNegativeInt,
        count: NonNegativeInt,
      }).check(isMaxLength(17)),
  ),
})
```

### OrchestrationV2SearchThreadsInput

Source: `packages/contracts/src/orchestrationV2.ts:3478`.

```text
Object({
  ...OrchestrationSearchThreadsInput.fields,
  olderSettledBefore: Optional(IsoDateTime),
  projectIds: Optional(Array(ProjectId)),
  environmentLabel: Optional(String),
  titlesOnly: Optional(Boolean),
})
```

### OrchestrationV2SearchThreadsResult

Source: `packages/contracts/src/orchestrationV2.ts:3489`.

```text
Object({
  ...OrchestrationSearchThreadsResult.fields,
  snapshotSequence: Optional(NonNegativeInt),
  threads: Optional(Array(OrchestrationV2ThreadShell)),
})
```

### OrchestrationV2SecretRequestDelivery

Source: `packages/contracts/src/orchestrationV2.ts:1434`.

```text
Const("file")
```

### OrchestrationV2SecretRequestFields

Source: `packages/contracts/src/orchestrationV2.ts:1436`.

```text
{
  type: Const("secret_request"),
  label: TrimmedNonEmptyString,
  reason: String,
  placeholder: Optional(String),
  delivery: Optional(OrchestrationV2SecretRequestDelivery),
  secretStatus: OrchestrationV2SecretRequestStatus,
}
```

### OrchestrationV2SecretRequestStatus

Source: `packages/contracts/src/orchestrationV2.ts:1425`.

```text
Enum([
  "pending",
  "saved",
  "declined",
  "cancelled",
])
```

### OrchestrationV2SessionCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:219`.

```text
Object({
  supportsMultipleProviderThreadsPerSession: Boolean,
  supportsModelSwitchInSession: Boolean,
  supportsProviderSwitchingViaHandoff: Boolean,
  supportsRuntimeModeSwitchInSession: Boolean,
  pendingRequestsSurviveRestart: Boolean,
})
```

### OrchestrationV2ShellSnapshot

Source: `packages/contracts/src/orchestrationV2.ts:2074`.

```text
Object({
  ...OrchestrationV2ThreadShellSnapshot.fields,
  projects: Array(OrchestrationProjectShell),
  olderSettled: Optional(OrchestrationV2OlderSettledThreads),
})
```

### OrchestrationV2ShellSnapshotJson

Source: `packages/contracts/src/orchestrationV2.ts:2578`.

```text
MergeFields(OrchestrationV2ShellSnapshot, {
    threads: Array(OrchestrationV2ThreadShellJson),
    archivedThreads: Array(OrchestrationV2ThreadShellJson),
  })
```

### OrchestrationV2ShellStreamItem

Source: `packages/contracts/src/orchestrationV2.ts:2081`.

```text
Union([
  Object({
    kind: Const("synchronized"),
  }),
  Object({
    kind: Const("snapshot"),
    snapshot: OrchestrationV2ShellSnapshot,
    resolvedRepositoryIdentityRoots: Optional(Array(String)),
  }),
  Object({
    kind: Const("project.updated"),
    sequence: NonNegativeInt,
    project: OrchestrationProjectShell,
  }),
  Object({
    kind: Const("project.removed"),
    sequence: NonNegativeInt,
    projectId: ProjectId,
  }),
  Object({
    kind: Const("thread.updated"),
    sequence: NonNegativeInt,
    location: Enum(["active", "archive"]),
    thread: OrchestrationV2ThreadShell,
  }),
  Object({
    kind: Const("thread.removed"),
    sequence: NonNegativeInt,
    location: Enum(["active", "archive"]),
    threadId: ThreadId,
  }),
])
```

### OrchestrationV2ShellThreadStatus

Source: `packages/contracts/src/orchestrationV2.ts:1902`.

```text
Union([
  Const("idle"),
  OrchestrationV2RunStatus,
])
```

### OrchestrationV2StopWorkflowError

Source: `packages/contracts/src/orchestrationV2.ts:3712`.

```text
Object({_tag: Const("OrchestrationV2StopWorkflowError"),
    threadId: ThreadId,
    subagentId: NodeId,
    reason: Enum(["not-running", "unsupported", "unavailable", "stop-failed"]),
    cause: Optional(Defect()),
  })
```

### OrchestrationV2StopWorkflowInput

Source: `packages/contracts/src/orchestrationV2.ts:3706`.

```text
Object({
  threadId: ThreadId,
  subagentId: NodeId,
})
```

### OrchestrationV2StoredEvent

Source: `packages/contracts/src/orchestrationV2.ts:2115`.

```text
Object({
  sequence: NonNegativeInt,
  commandId: Nullable(CommandId),
  event: OrchestrationV2DomainEvent,
})
```

### OrchestrationV2StoredEventJson

Source: `packages/contracts/src/orchestrationV2.ts:2746`.

```text
Object({
  sequence: NonNegativeInt,
  commandId: Nullable(CommandId),
  event: OrchestrationV2DomainEventJson,
})
```

### OrchestrationV2StreamingCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:253`.

```text
Object({
  streamsAssistantText: Boolean,
  streamsReasoning: Boolean,
  streamsToolOutput: Boolean,
  streamsPlanText: Boolean,
  emitsMessageCompleted: Boolean,
})
```

### OrchestrationV2Subagent

Source: `packages/contracts/src/orchestrationV2.ts:791`.

```text
Object({
  id: NodeId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  parentNodeId: NodeId,
  origin: Enum(["provider_native", "app_owned"]),
  createdBy: OrchestrationV2Actor,
  driver: ProviderDriverKind,
  providerInstanceId: ProviderInstanceId,
  providerThreadId: Nullable(ProviderThreadId),
  childThreadId: Nullable(ThreadId),
  nativeTaskRef: Nullable(OrchestrationV2ProviderRef),
  prompt: String,
  title: Nullable(String),
  model: Nullable(String),
  modelSelection: Optional(ModelSelection),
  completionWake: Optional(Enum(["always", "settled_only"])),
  completionDelivery: Optional(OrchestrationV2DelegatedCompletionTaskDelivery),
  status: Enum([
    "idle",
    "pending",
    "running",
    "waiting",
    "completed",
    "failed",
    "cancelled",
    "interrupted",
  ]),
  progress: Optional(String),
  workflow: Optional(OrchestrationV2SubagentWorkflow),
  result: Nullable(String),
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
  updatedAt: DateTimeUtc,
  promptOmitted: DedupedBodyMarker,
  resultOmitted: DedupedBodyMarker,
})
```

### OrchestrationV2SubagentCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:292`.

```text
Object({
  supportsSubagents: Boolean,
  exposesSubagentThreadIds: Boolean,
  emitsSubagentLifecycle: Boolean,
  canWaitForSubagents: Boolean,
  canCloseSubagents: Boolean,
  canForkSubagentThread: Boolean,
})
```

### OrchestrationV2SubagentJson

Source: `packages/contracts/src/orchestrationV2.ts:2176`.

```text
MergeFields(OrchestrationV2Subagent, {
  startedAt: Nullable(DateTimeUtcFromString),
  completedAt: Nullable(DateTimeUtcFromString),
  updatedAt: DateTimeUtcFromString,
})
```

### OrchestrationV2SubagentWorkflow

Source: `packages/contracts/src/orchestrationV2.ts:772`.

```text
Object({
  name: Optional(TrimmedNonEmptyString),
  runHandles: Optional(OrchestrationV2WorkflowRunHandles),
  phases: Array(OrchestrationV2WorkflowPhase),
  agents: Array(OrchestrationV2WorkflowAgent),
  totalTokens: Optional(NonNegativeInt),
  toolCalls: Optional(NonNegativeInt),
  durationMs: Optional(NonNegativeInt),
})
```

### OrchestrationV2SubscribeCommandOutputInput

Source: `packages/contracts/src/orchestrationV2.ts:3339`.

```text
Object({
  threadId: ThreadId,
  itemId: TurnItemId,
})
```

### OrchestrationV2SubscribeShellInput

Source: `packages/contracts/src/orchestrationV2.ts:3434`.

```text
Object({
  afterSequence: Optional(NonNegativeInt),
  requestCompletionMarker: Optional(Boolean),
  omitIdleSubagents: Optional(Boolean),
  omitOlderSettled: Optional(Boolean),
})
```

### OrchestrationV2SubscribeThreadInput

Source: `packages/contracts/src/orchestrationV2.ts:3532`.

```text
Object({
  threadId: ThreadId,
  afterSequence: Optional(NonNegativeInt),
  requestCompletionMarker: Optional(Boolean),
  acceptBoundedSnapshot: Optional(Boolean),
  acceptCompactTurnItems: Optional(Boolean),
  acceptDedupedBodies: Optional(Boolean),
  acceptCompactIds: Optional(Boolean),
})
```

### OrchestrationV2ThreadBoundedSnapshot

Source: `packages/contracts/src/orchestrationV2.ts:3561`.

```text
withRecordIdTable(
  Object({
    snapshotSequence: NonNegativeInt,
    projection: OrchestrationV2ThreadProjection,
    historyCursor: Nullable(TrimmedNonEmptyString),
    hasMoreHistory: Boolean,
    latestLocalTurnOrdinal: Nullable(NonNegativeInt),
    turnItemsOmitLocalVisible: Optional(Const(true)),
    idTable: IdTableMarker,
  }),
  container="projection",
)
```

### OrchestrationV2ThreadCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:228`.

```text
Object({
  canCreateEmptyThread: Boolean,
  canReadThreadSnapshot: Boolean,
  canRollbackThread: Boolean,
  canForkThread: Boolean,
  canForkFromTurn: Boolean,
  canForkFromSubagentThread: Boolean,
  exposesNativeThreadId: Boolean,
})
```

### OrchestrationV2ThreadDetailSnapshot

Source: `packages/contracts/src/orchestrationV2.ts:3550`.

```text
Object({
  snapshotSequence: NonNegativeInt,
  projection: OrchestrationV2ThreadProjection,
  historyCursor: Optional(Nullable(TrimmedNonEmptyString)),
  hasMoreHistory: Optional(Boolean),
  latestLocalTurnOrdinal: Optional(Nullable(NonNegativeInt)),
})
```

### OrchestrationV2ThreadFamily

Source: `packages/contracts/src/orchestrationV2.ts:3460`.

```text
Object({
  snapshotSequence: NonNegativeInt,
  threads: Array(OrchestrationV2ThreadShell),
})
```

### OrchestrationV2ThreadFindMatch

Source: `packages/contracts/src/orchestrationV2.ts:3856`.

```text
Object({
  entryId: TrimmedNonEmptyString,
  runId: Nullable(RunId),
  occurrence: NonNegativeInt,
})
```

### OrchestrationV2ThreadForkSourcePoint

Source: `packages/contracts/src/orchestrationV2.ts:144`.

```text
Union([
  Object({ type: Const("latest_stable") }),
  Object({ type: Const("run"), runId: RunId }),
  Object({ type: Const("checkpoint"), checkpointId: CheckpointId }),
])
```

### OrchestrationV2ThreadHistoryOrigin

Source: `packages/contracts/src/orchestrationV2.ts:98`.

```text
Enum(["native", "v1_import"])
```

### OrchestrationV2ThreadHistoryPage

Source: `packages/contracts/src/orchestrationV2.ts:3580`.

```text
withRecordIdTable(
  Object({
    snapshotSequence: NonNegativeInt,
    items: projectedTurnItemArray(
      OrchestrationV2ProjectedTurnItem,
      MergeFields(OrchestrationV2ProjectedTurnItem, {
        item: ForwardCompatibleUnion(OrchestrationV2TurnItem.members, "type"),
      }),
    ),
    nextCursor: Nullable(TrimmedNonEmptyString),
    hasMoreHistory: Boolean,
    idTable: IdTableMarker,
  }),
  container=root,
)
```

### OrchestrationV2ThreadLaunchError

Source: `packages/contracts/src/orchestrationV2.ts:3696`.

```text
Object({_tag: Const("OrchestrationV2ThreadLaunchError"),
    commandId: CommandId,
    projectId: ProjectId,
    message: String,
    cause: Optional(Defect()),
  })
```

### OrchestrationV2ThreadLaunchInput

Source: `packages/contracts/src/orchestrationV2.ts:3393`.

```text
Object({
  commandId: CommandId,
  creationSource: Optional(OrchestrationV2CreationSource),
  threadId: Optional(ThreadId),
  reuseExistingThread: Optional(Boolean),
  projectId: ProjectId,
  title: TrimmedNonEmptyString,
  generateTitle: Optional(Boolean),
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
  initialMessage: Optional(
    Object({
      messageId: Optional(MessageId),
      text: String,
      context: Optional(OrchestrationMessageContext),
      attachments: Array(ChatAttachment),
    }),
})
```

### OrchestrationV2ThreadLaunchResult

Source: `packages/contracts/src/orchestrationV2.ts:3416`.

```text
Object({
  threadId: ThreadId,
  projection: OrchestrationV2ThreadProjection,
  resumed: Boolean,
})
```

### OrchestrationV2ThreadLaunchWorkspaceStrategy

Source: `packages/contracts/src/orchestrationV2.ts:607`.

```text
Union([
  Object({
    type: Const("root"),
    branch: Optional(TrimmedNonEmptyString),
  }),
  Object({
    type: Const("existing_worktree"),
    worktreePath: TrimmedNonEmptyString,
    branch: Optional(TrimmedNonEmptyString),
  }),
  Object({
    type: Const("worktree"),
    baseRef: TrimmedNonEmptyString,
    branch: Optional(TrimmedNonEmptyString),
    startFromOrigin: Optional(Boolean),
  }),
])
```

### OrchestrationV2ThreadOpens

Source: `packages/contracts/src/orchestrationV2.ts:386`.

```text
Object({
  openedAt: Finite,
  score: Finite.check(isGreaterThanOrEqualTo(0)),
  picks: Array(
    Object({ query: String.check(isMaxLength(32)), at: Finite }).check(isMaxLength(3)),
})
```

### OrchestrationV2ThreadProjection

Source: `packages/contracts/src/orchestrationV2.ts:1866`.

```text
Object({
  thread: OrchestrationV2AppThread,
  runs: Array(OrchestrationV2Run),
  attempts: Array(OrchestrationV2RunAttempt),
  nodes: Array(OrchestrationV2ExecutionNode),
  subagents: Array(OrchestrationV2Subagent),
  providerSessions: Array(OrchestrationV2ProviderSession),
  providerThreads: Array(OrchestrationV2ProviderThread),
  providerTurns: Array(OrchestrationV2ProviderTurn),
  runtimeRequests: Array(OrchestrationV2RuntimeRequest),
  messages: Array(OrchestrationV2ConversationMessage),
  plans: Array(OrchestrationV2PlanArtifact),
  turnItems: turnItemArray(OrchestrationV2TurnItem),
  checkpointScopes: Array(OrchestrationV2CheckpointScope),
  checkpoints: Array(OrchestrationV2Checkpoint),
  contextHandoffs: Array(OrchestrationV2ContextHandoff),
  contextTransfers: Array(OrchestrationV2ContextTransfer),
  visibleTurnItems: projectedTurnItemArray(
    OrchestrationV2ProjectedTurnItem,
    MergeFields(OrchestrationV2ProjectedTurnItem, {
      item: ForwardCompatibleUnion(OrchestrationV2TurnItem.members, "type"),
    }),
  ),
  updatedAt: DateTimeUtc,
})
```

### OrchestrationV2ThreadProjectionJson

Source: `packages/contracts/src/orchestrationV2.ts:2499`.

```text
MergeFields(OrchestrationV2ThreadProjection, {
    thread: OrchestrationV2AppThreadJson,
    runs: Array(OrchestrationV2RunJson),
    attempts: Array(OrchestrationV2RunAttemptJson),
    nodes: Array(OrchestrationV2ExecutionNodeJson),
    subagents: Array(OrchestrationV2SubagentJson),
    providerSessions: Array(OrchestrationV2ProviderSessionJson),
    providerThreads: Array(OrchestrationV2ProviderThreadJson),
    providerTurns: Array(OrchestrationV2ProviderTurnJson),
    runtimeRequests: Array(OrchestrationV2RuntimeRequestJson),
    messages: Array(OrchestrationV2ConversationMessageJson),
    plans: Array(OrchestrationV2PlanArtifact),
    turnItems: turnItemArray(OrchestrationV2TurnItemJson),
    checkpointScopes: Array(OrchestrationV2CheckpointScopeJson),
    checkpoints: Array(OrchestrationV2CheckpointJson),
    contextHandoffs: Array(OrchestrationV2ContextHandoffJson),
    contextTransfers: Array(OrchestrationV2ContextTransferJson),
    visibleTurnItems: projectedTurnItemArray(
      OrchestrationV2ProjectedTurnItemJson,
      MergeFields(OrchestrationV2ProjectedTurnItemJson, {
        item: ForwardCompatibleUnion(OrchestrationV2TurnItemJson.members, "type"),
      }),
    ),
    updatedAt: DateTimeUtcFromString,
  })
```

### OrchestrationV2ThreadShell

Source: `packages/contracts/src/orchestrationV2.ts:1927`.

```text
Object({
  ...OrchestrationV2CreationFields,
  id: ThreadId,
  projectId: ProjectId,
  title: String,
  providerInstanceId: ProviderInstanceId,
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  branch: Nullable(TrimmedNonEmptyString),
  worktreePath: Nullable(TrimmedNonEmptyString),
  linkedPullRequest: Optional(Nullable(ThreadLinkedPullRequest)),
  pullRequests: Optional(Array(ThreadPullRequestLink)),
  branchPullRequest: Optional(Nullable(ThreadLinkedPullRequest)),
  lineage: OrchestrationV2AppThreadLineage,
  forkedFrom: Nullable(OrchestrationV2AppThread.fields.forkedFrom),
  activeProviderThreadId: Nullable(ProviderThreadId),
  historyOrigin: Optional(OrchestrationV2ThreadHistoryOrigin),
  latestRunId: Nullable(RunId),
  latestRunRequestedAt: Optional(Nullable(DateTimeUtc)),
  latestRunStartedAt: Optional(Nullable(DateTimeUtc)),
  latestRunCompletedAt: Optional(Nullable(DateTimeUtc)),
  latestRunStopRequested: Optional(Boolean),
  activeRunId: Nullable(RunId),
  activityRunStartedAt: Optional(Nullable(DateTimeUtc)),
  activityRunStatus: Optional(
    Nullable(Enum(["preparing", "starting", "running", "waiting"])),
  ),
  blockedOnThreadWait: Optional(Boolean),
  status: OrchestrationV2ShellThreadStatus,
  lastError: Optional(Nullable(String)),
  lastErrorClass: Optional(Nullable(OrchestrationV2ProviderFailureClass)),
  lastErrorCode: Optional(Nullable(String)),
  usageLimitResetAt: Optional(Nullable(IsoDateTime)),
  pendingRuntimeRequest: Nullable(OrchestrationV2PendingRuntimeRequestSummary),
  pendingGuestMessageCount: Optional(NonNegativeInt),
  latestVisibleMessage: Nullable(OrchestrationV2LatestVisibleMessageSummary),
  latestUserMessageAt: Nullable(DateTimeUtc),
  latestUserAuthoredMessageAt: Optional(Nullable(DateTimeUtc)),
  hasActionableProposedPlan: Boolean,
  pendingBackgroundTasks: Optional(Array(OrchestrationV2PendingBackgroundTask)),
  providerInstanceHistory: Optional(Array(ProviderInstanceId)),
  goal: Optional(Nullable(OrchestrationV2ProviderGoal)),
  itemCount: NonNegativeInt,
  visibleItemCount: NonNegativeInt,
  createdAt: DateTimeUtc,
  updatedAt: DateTimeUtc,
  archivedAt: Nullable(DateTimeUtc),
  settledOverride: Nullable(Enum(["settled", "active"])),
  settledAt: Nullable(DateTimeUtc),
  unsettledAt: Optional(Nullable(DateTimeUtc)),
  snoozedUntil: Optional(Nullable(DateTimeUtc)),
  snoozedAt: Optional(Nullable(DateTimeUtc)),
  limitRecovery: Optional(Nullable(OrchestrationV2LimitRecovery)),
  pinnedAt: Optional(Nullable(DateTimeUtc)),
  autoSettleDisabledAt: Optional(Nullable(DateTimeUtc)),
  pinOrderKey: Optional(Nullable(TrimmedNonEmptyString)),
  activeOrderKey: Optional(Nullable(TrimmedNonEmptyString)),
  lastVisitedAt: Optional(Nullable(DateTimeUtc)),
  opens: Optional(LenientThreadOpens),
  titleRegeneration: Optional(
    Nullable(
      Object({
        requestId: CommandId,
        startedAt: DateTimeUtc,
      }),
  ),
  deletedAt: Nullable(DateTimeUtc),
})
```

### OrchestrationV2ThreadShellJson

Source: `packages/contracts/src/orchestrationV2.ts:2546`.

```text
MergeFields(OrchestrationV2ThreadShell, {
  latestRunRequestedAt: Optional(Nullable(DateTimeUtcFromString)),
  latestRunStartedAt: Optional(Nullable(DateTimeUtcFromString)),
  latestRunCompletedAt: Optional(Nullable(DateTimeUtcFromString)),
  activityRunStartedAt: Optional(Nullable(DateTimeUtcFromString)),
  pendingRuntimeRequest: Nullable(OrchestrationV2PendingRuntimeRequestSummaryJson),
  latestVisibleMessage: Nullable(OrchestrationV2LatestVisibleMessageSummaryJson),
  latestUserMessageAt: Nullable(DateTimeUtcFromString),
  latestUserAuthoredMessageAt: Optional(Nullable(DateTimeUtcFromString)),
  createdAt: DateTimeUtcFromString,
  updatedAt: DateTimeUtcFromString,
  archivedAt: Nullable(DateTimeUtcFromString),
  settledAt: Nullable(DateTimeUtcFromString),
  unsettledAt: Optional(Nullable(DateTimeUtcFromString)),
  snoozedUntil: Optional(Nullable(DateTimeUtcFromString)),
  snoozedAt: Optional(Nullable(DateTimeUtcFromString)),
  pinnedAt: Optional(Nullable(DateTimeUtcFromString)),
  autoSettleDisabledAt: Optional(Nullable(DateTimeUtcFromString)),
  lastVisitedAt: Optional(Nullable(DateTimeUtcFromString)),
  titleRegeneration: Optional(
    Nullable(
      Object({
        requestId: CommandId,
        startedAt: DateTimeUtcFromString,
      }),
  ),
  deletedAt: Nullable(DateTimeUtcFromString),
})
```

### OrchestrationV2ThreadShellSnapshot

Source: `packages/contracts/src/orchestrationV2.ts:2066`.

```text
Object({
  schemaVersion: PositiveInt,
  snapshotSequence: NonNegativeInt,
  threads: Array(OrchestrationV2ThreadShell),
  archivedThreads: Array(OrchestrationV2ThreadShell),
})
```

### OrchestrationV2ThreadStreamItem

Source: `packages/contracts/src/orchestrationV2.ts:3642`.

```text
withRecordIdTable(
  Union([
    Object({
      kind: Const("synchronized"),
    }),
    Object({
      kind: Const("snapshot"),
      snapshotSequence: NonNegativeInt,
      projection: OrchestrationV2ThreadProjection,
      historyCursor: Optional(Nullable(TrimmedNonEmptyString)),
      hasMoreHistory: Optional(Boolean),
      latestLocalTurnOrdinal: Optional(Nullable(NonNegativeInt)),
      turnItemsOmitLocalVisible: Optional(Const(true)),
      idTable: IdTableMarker,
    }),
    Object({
      kind: Const("event"),
      sequence: NonNegativeInt,
      event: OrchestrationV2DomainEvent,
    }),
    OrchestrationV2UnknownThreadStreamEvent,
  ]),
  container="projection" for snapshot; unchanged for other kinds,
)
```

### OrchestrationV2ThreadTranscript

Source: `packages/contracts/src/orchestrationV2.ts:1894`.

```text
Object({
  threadId: ThreadId,
  title: String,
  updatedAt: DateTimeUtc,
  items: Array(OrchestrationV2ProjectedTurnItem),
})
```

### OrchestrationV2ToolCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:262`.

```text
Object({
  exposesToolItemIds: Boolean,
  emitsToolStarted: Boolean,
  emitsToolCompleted: Boolean,
  emitsToolOutput: Boolean,
  supportsMcpTools: Boolean,
  supportsDynamicToolCallbacks: Boolean,
})
```

### OrchestrationV2TurnCapabilities

Source: `packages/contracts/src/orchestrationV2.ts:239`.

```text
Object({
  exposesNativeTurnId: Boolean,
  emitsTurnStarted: Boolean,
  emitsTurnCompleted: Boolean,
  supportsInterrupt: Boolean,
  supportsActiveSteering: Boolean,
  activeSteeringInterruptsTools: Optional(Boolean),
  supportsSteeringByInterruptRestart: Boolean,
  supportsQueuedMessages: Boolean,
  terminalStatusQuality: Enum(["strong", "weak", "none"]),
})
```

### OrchestrationV2TurnItem

Source: `packages/contracts/src/orchestrationV2.ts:1445`.

```text
Union([
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("notification"),
    ...OrchestrationV2Notification.fields,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    ...OrchestrationV2CreationFields,
    type: Const("user_message"),
    messageId: MessageId,
    scheduledTaskId: Optional(ScheduledTaskId),
    senderThreadId: Optional(ThreadId),
    idleCompaction: Optional(OrchestrationV2IdleCompaction),
    workflowScript: OrchestrationV2ConversationMessage.fields.workflowScript,
    inputIntent: OrchestrationV2UserMessageInputIntent,
    text: String,
    textOmitted: DedupedBodyMarker,
    context: Optional(OrchestrationMessageContext),
    attachments: Array(ChatAttachment),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("assistant_message"),
    messageId: MessageId,
    text: String,
    textOmitted: DedupedBodyMarker,
    attachments: Optional(Array(ChatAttachment)),
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("reasoning"),
    text: String,
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("proposed_plan"),
    planId: PlanId,
    markdown: String,
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("todo_list"),
    planId: PlanId,
    steps: Array(OrchestrationV2PlanStep),
    explanation: Optional(String),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("user_input_request"),
    requestId: RuntimeRequestId,
    questions: Array(OrchestrationV2UserInputQuestion),
    questionAnswer: Optional(UserInputAttachmentAnswerPayload),
    responseMode: Optional(Const("message")),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("file_change"),
    fileName: TrimmedNonEmptyString,
    additions: Optional(NonNegativeInt),
    deletions: Optional(NonNegativeInt),
    diffStr: Optional(String),
    oldStr: Optional(String),
    newStr: Optional(String),
    changes: Optional(Array(OrchestrationV2FileChangeDetail)),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("command_execution"),
    input: String,
    output: Optional(String),
    outputOmitted: Optional(Boolean),
    outputIndicatesFailure: Optional(Boolean),
    exitCode: Optional(Int),
    monitor: Optional(Boolean),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("file_search"),
    pattern: Optional(String),
    results: Optional(Array(OrchestrationV2FileSearchResult)),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("web_search"),
    patterns: Optional(Array(String)),
    results: Optional(Array(OrchestrationV2WebSearchResult)),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("approval_request"),
    requestId: RuntimeRequestId,
    requestKind: ProviderRequestKind,
    prompt: Optional(String),
    reason: Optional(String),
    fullAccessOverride: Optional(ProviderApprovalFullAccessOverride),
    appName: Optional(String),
    options: Optional(Array(ProviderApprovalOption)),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("checkpoint"),
    checkpointId: CheckpointId,
    scopeId: CheckpointScopeId,
    files: Array(OrchestrationV2CheckpointFileSummary),
    filesOmitted: DedupedBodyMarker,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("run_interrupt_request"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("run_interrupt_result"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("system_notice"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("error"),
    failure: OrchestrationV2ProviderFailure,
    retry: Optional(OrchestrationV2ProviderRetry),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("compaction"),
    driver: Nullable(ProviderDriverKind),
    summary: Optional(String),
    beforeTokenCount: Optional(NonNegativeInt),
    afterTokenCount: Optional(NonNegativeInt),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("handoff"),
    contextHandoffId: ContextHandoffId,
    fromProviderThreadIds: Array(ProviderThreadId),
    toProviderThreadId: ProviderThreadId,
    fromProviderInstanceIds: Array(ProviderInstanceId),
    toProviderInstanceId: ProviderInstanceId,
    fromModelSelections: Optional(Array(ModelSelection)),
    toModel: Optional(String),
    strategy: Enum([
      "delta_since_target_last_seen",
      "fork_delta_summary",
      "full_thread_summary",
      "checkpoint_summary",
      "manual_context",
    ]),
    summary: Optional(String),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("fork"),
    source: Union([
      Object({ type: Const("run"), threadId: ThreadId, runId: RunId }),
      Object({ type: Const("node"), nodeId: NodeId }),
      Object({
        type: Const("provider_thread"),
        providerThreadId: ProviderThreadId,
        providerTurnId: Optional(ProviderTurnId),
      }),
    ]),
    targetThreadId: ThreadId,
    providerThreadId: Optional(ProviderThreadId),
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("thread_created"),
    targetThreadId: ThreadId,
    targetRunId: Nullable(RunId),
    targetProviderInstanceId: ProviderInstanceId,
    targetModel: TrimmedNonEmptyString,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    ...OrchestrationV2SecretRequestFields,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("subagent"),
    subagentId: NodeId,
    origin: Enum(["provider_native", "app_owned"]),
    driver: ProviderDriverKind,
    providerInstanceId: ProviderInstanceId,
    childThreadId: Nullable(ThreadId),
    prompt: String,
    progress: Optional(String),
    result: Nullable(String),
    promptOmitted: DedupedBodyMarker,
    resultOmitted: DedupedBodyMarker,
  }),
  Object({
    ...OrchestrationV2TurnItemBaseFields,
    type: Const("dynamic_tool"),
    toolName: Nullable(TrimmedNonEmptyString),
    viewedImagePath: Optional(TrimmedNonEmptyString),
    input: Unknown,
    output: Optional(Unknown),
    outputOmitted: Optional(Boolean),
  }),
])
```

### OrchestrationV2TurnItemBaseFields

Source: `packages/contracts/src/orchestrationV2.ts:1389`.

```text
{
  toolNonExecutionKind: Optional(String),
  toolSurface: Optional(ToolActivitySurface),
  toolIcon: Optional(ToolActivityIcon),
  toolSource: Optional(ToolActivitySource),
  id: TurnItemId,
  threadId: ThreadId,
  runId: Nullable(RunId),
  nodeId: Nullable(NodeId),
  providerThreadId: Nullable(ProviderThreadId),
  providerTurnId: Nullable(ProviderTurnId),
  nativeItemRef: Nullable(OrchestrationV2ProviderRef),
  parentItemId: Nullable(TurnItemId),
  ordinal: NonNegativeInt,
  status: OrchestrationV2TurnItemStatus,
  title: Nullable(String),
  startedAt: Nullable(DateTimeUtc),
  completedAt: Nullable(DateTimeUtc),
  updatedAt: DateTimeUtc,
}
```

### OrchestrationV2TurnItemJson

Source: `packages/contracts/src/orchestrationV2.ts:2282`.

```text
Union([
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("notification"),
    ...OrchestrationV2Notification.fields,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    ...OrchestrationV2CreationFields,
    type: Const("user_message"),
    messageId: MessageId,
    scheduledTaskId: Optional(ScheduledTaskId),
    senderThreadId: Optional(ThreadId),
    idleCompaction: Optional(OrchestrationV2IdleCompaction),
    workflowScript: OrchestrationV2ConversationMessage.fields.workflowScript,
    inputIntent: OrchestrationV2UserMessageInputIntent,
    text: String,
    context: Optional(OrchestrationMessageContext),
    attachments: Array(ChatAttachment),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("assistant_message"),
    messageId: MessageId,
    text: String,
    attachments: Optional(Array(ChatAttachment)),
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("reasoning"),
    text: String,
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("proposed_plan"),
    planId: PlanId,
    markdown: String,
    streaming: Boolean,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("todo_list"),
    planId: PlanId,
    steps: Array(OrchestrationV2PlanStep),
    explanation: Optional(String),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("user_input_request"),
    requestId: RuntimeRequestId,
    questions: Array(OrchestrationV2UserInputQuestion),
    questionAnswer: Optional(UserInputAttachmentAnswerPayload),
    responseMode: Optional(Const("message")),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("file_change"),
    fileName: TrimmedNonEmptyString,
    additions: Optional(NonNegativeInt),
    deletions: Optional(NonNegativeInt),
    diffStr: Optional(String),
    oldStr: Optional(String),
    newStr: Optional(String),
    changes: Optional(Array(OrchestrationV2FileChangeDetail)),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("command_execution"),
    input: String,
    output: Optional(String),
    outputOmitted: Optional(Boolean),
    outputIndicatesFailure: Optional(Boolean),
    exitCode: Optional(Int),
    monitor: Optional(Boolean),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("file_search"),
    pattern: Optional(String),
    results: Optional(Array(OrchestrationV2FileSearchResult)),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("web_search"),
    patterns: Optional(Array(String)),
    results: Optional(Array(OrchestrationV2WebSearchResult)),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("approval_request"),
    requestId: RuntimeRequestId,
    requestKind: ProviderRequestKind,
    prompt: Optional(String),
    reason: Optional(String),
    fullAccessOverride: Optional(ProviderApprovalFullAccessOverride),
    appName: Optional(String),
    options: Optional(Array(ProviderApprovalOption)),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("checkpoint"),
    checkpointId: CheckpointId,
    scopeId: CheckpointScopeId,
    files: Array(OrchestrationV2CheckpointFileSummary),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("run_interrupt_request"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("run_interrupt_result"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("system_notice"),
    message: String,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("error"),
    failure: OrchestrationV2ProviderFailure,
    retry: Optional(OrchestrationV2ProviderRetry),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("compaction"),
    driver: Nullable(ProviderDriverKind),
    summary: Optional(String),
    beforeTokenCount: Optional(NonNegativeInt),
    afterTokenCount: Optional(NonNegativeInt),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("handoff"),
    contextHandoffId: ContextHandoffId,
    fromProviderThreadIds: Array(ProviderThreadId),
    toProviderThreadId: ProviderThreadId,
    fromProviderInstanceIds: Array(ProviderInstanceId),
    toProviderInstanceId: ProviderInstanceId,
    fromModelSelections: Optional(Array(ModelSelection)),
    toModel: Optional(String),
    strategy: Enum([
      "delta_since_target_last_seen",
      "fork_delta_summary",
      "full_thread_summary",
      "checkpoint_summary",
      "manual_context",
    ]),
    summary: Optional(String),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("fork"),
    source: Union([
      Object({ type: Const("run"), threadId: ThreadId, runId: RunId }),
      Object({ type: Const("node"), nodeId: NodeId }),
      Object({
        type: Const("provider_thread"),
        providerThreadId: ProviderThreadId,
        providerTurnId: Optional(ProviderTurnId),
      }),
    ]),
    targetThreadId: ThreadId,
    providerThreadId: Optional(ProviderThreadId),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("thread_created"),
    targetThreadId: ThreadId,
    targetRunId: Nullable(RunId),
    targetProviderInstanceId: ProviderInstanceId,
    targetModel: TrimmedNonEmptyString,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    ...OrchestrationV2SecretRequestFields,
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("subagent"),
    subagentId: NodeId,
    origin: Enum(["provider_native", "app_owned"]),
    driver: ProviderDriverKind,
    providerInstanceId: ProviderInstanceId,
    childThreadId: Nullable(ThreadId),
    prompt: String,
    progress: Optional(String),
    result: Nullable(String),
  }),
  Object({
    ...OrchestrationV2TurnItemJsonBaseFields,
    type: Const("dynamic_tool"),
    toolName: Nullable(TrimmedNonEmptyString),
    viewedImagePath: Optional(TrimmedNonEmptyString),
    input: Unknown,
    output: Optional(Unknown),
    outputOmitted: Optional(Boolean),
  }),
])
```

### OrchestrationV2TurnItemJsonBaseFields

Source: `packages/contracts/src/orchestrationV2.ts:2275`.

```text
{
  ...OrchestrationV2TurnItemBaseFields,
  startedAt: Nullable(DateTimeUtcFromString),
  completedAt: Nullable(DateTimeUtcFromString),
  updatedAt: DateTimeUtcFromString,
}
```

### OrchestrationV2TurnItemStatus

Source: `packages/contracts/src/orchestrationV2.ts:1322`.

```text
Enum([
  "idle",
  "pending",
  "running",
  "waiting",
  "completed",
  "failed",
  "cancelled",
  "interrupted",
])
```

### OrchestrationV2UnknownThreadStreamEvent

Source: `packages/contracts/src/orchestrationV2.ts:3610`.

```text
decode-only compatibility branch: incoming {kind:"event",sequence,event:{type:string,payload?:JSON}} for an unknown event type. Never emit kind:"unknown-event" from the server.
```

### OrchestrationV2UserInputQuestion

Source: `packages/contracts/src/orchestrationV2.ts:1235`.

```text
Object({
  id: TrimmedNonEmptyString,
  header: TrimmedNonEmptyString,
  question: TrimmedNonEmptyString,
  options: Array(
    Object({
      label: TrimmedNonEmptyString,
      description: TrimmedNonEmptyString,
      value: Optional(String),
      preview: Optional(String),
    }),
  multiSelect: Optional(Boolean),
  allowCustomAnswer: Optional(Boolean),
  initialAnswer: Optional(String),
  required: Optional(Boolean),
})
```

### OrchestrationV2UserMessageInputIntent

Source: `packages/contracts/src/orchestrationV2.ts:1380`.

```text
Enum([
  "turn_start",
  "queued_turn",
  "steer",
  "promoted_queued_to_steer",
])
```

### OrchestrationV2WebSearchResult

Source: `packages/contracts/src/orchestrationV2.ts:1418`.

```text
Object({
  title: Optional(String),
  url: Optional(TrimmedNonEmptyString),
  snippet: Optional(String),
})
```

### OrchestrationV2WorkflowAgent

Source: `packages/contracts/src/orchestrationV2.ts:744`.

```text
Object({
  index: NonNegativeInt,
  label: TrimmedNonEmptyString,
  agentId: Optional(TrimmedNonEmptyString),
  childThreadId: Optional(ThreadId),
  state: Enum(["queued", "running", "completed", "failed"]),
  phaseIndex: Optional(NonNegativeInt),
  phaseTitle: Optional(TrimmedNonEmptyString),
  model: Optional(TrimmedNonEmptyString),
  attempt: Optional(NonNegativeInt),
  totalTokens: Optional(NonNegativeInt),
  toolCalls: Optional(NonNegativeInt),
  durationMs: Optional(NonNegativeInt),
  queuedAt: Optional(NonNegativeInt),
  startedAt: Optional(NonNegativeInt),
  completedAt: Optional(NonNegativeInt),
  prompt: Optional(NonEmptyString),
  result: Optional(NonEmptyString),
})
```

### OrchestrationV2WorkflowPhase

Source: `packages/contracts/src/orchestrationV2.ts:737`.

```text
Object({
  index: NonNegativeInt,
  title: TrimmedNonEmptyString,
})
```

### OrchestrationV2WorkflowRunHandles

Source: `packages/contracts/src/orchestrationV2.ts:765`.

```text
Object({
  runId: Optional(TrimmedNonEmptyString),
  transcriptDir: Optional(TrimmedNonEmptyString),
  scriptPath: Optional(TrimmedNonEmptyString),
})
```

### OrchestrationV2WorktreeThreadIds

Source: `packages/contracts/src/orchestrationV2.ts:3527`.

```text
Object({
  threadIds: Array(ThreadId),
})
```

### OtlpEndpointCheckInput

Source: `packages/contracts/src/server.ts:304`.

```text
Object({
  signal: OtlpSignal,
  url: TrimmedNonEmptyString.check(isPattern(/^https?:\/\/./i)),
})
```

### OtlpEndpointCheckResult

Source: `packages/contracts/src/server.ts:310`.

```text
Union([
  TaggedObject("Accepted", { latencyMs: Number }),
  TaggedObject("Rejected", { status: Number }),
  TaggedObject("Unreachable", { timedOut: Boolean }),
])
```

### OtlpEndpointSource

Source: `packages/contracts/src/server.ts:280`.

```text
Enum(["environment", "bootstrap", "settings"])
```

### OtlpSignal

Source: `packages/contracts/src/server.ts:301`.

```text
Enum(["traces", "metrics", "logs"])
```

### PREVIEW_URL_MAX_LENGTH

Source: `packages/contracts/src/preview.ts:5`.

```text
2_048
```

### PREVIEW_VIEWPORT_MAX_AREA

Source: `packages/contracts/src/preview.ts:20`.

```text
3840 * 2160
```

### PREVIEW_VIEWPORT_MAX_DIMENSION

Source: `packages/contracts/src/preview.ts:19`.

```text
3840
```

### PREVIEW_VIEWPORT_MIN_DIMENSION

Source: `packages/contracts/src/preview.ts:18`.

```text
240
```

### PREVIEW_VIEWPORT_PRESET_IDS

Source: `packages/contracts/src/preview.ts:48`.

```text
[
  "iphone-se",
  "iphone-xr",
  "iphone-12-pro",
  "iphone-14-pro-max",
  "pixel-7",
  "samsung-galaxy-s8-plus",
  "samsung-galaxy-s20-ultra",
  "ipad-mini",
  "ipad-air",
  "ipad-pro",
  "surface-pro-7",
  "surface-duo",
  "galaxy-z-fold-5",
  "asus-zenbook-fold",
  "samsung-galaxy-a51-71",
  "nest-hub",
  "nest-hub-max",
]
```

### PREVIEW_ZOOM_LEVELS

Source: `packages/contracts/src/preview.ts:104`.

```text
[
  0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
]
```

### PROJECT_CLONE_DETAIL_MAX_LENGTH

Source: `packages/contracts/src/projectClone.ts:10`.

```text
200
```

### PROJECT_CLONE_ERROR_MAX_LENGTH

Source: `packages/contracts/src/projectClone.ts:11`.

```text
1000
```

### PROJECT_READ_FILE_PATH_MAX_LENGTH

Source: `packages/contracts/src/project.ts:21`.

```text
512
```

### PROJECT_SEARCH_CONTENTS_MAX_LIMIT

Source: `packages/contracts/src/project.ts:19`.

```text
500
```

### PROJECT_SEARCH_ENTRIES_MAX_LIMIT

Source: `packages/contracts/src/project.ts:18`.

```text
200
```

### PROJECT_WRITE_FILE_PATH_MAX_LENGTH

Source: `packages/contracts/src/project.ts:20`.

```text
512
```

### PROVIDER_SEND_TURN_MAX_ATTACHMENTS

Source: `packages/contracts/src/chatAttachment.ts:14`.

```text
100
```

### PROVIDER_SEND_TURN_MAX_FILE_BYTES

Source: `packages/contracts/src/chatAttachment.ts:17`.

```text
50 * 1024 * 1024
```

### PROVIDER_SEND_TURN_MAX_IMAGE_BYTES

Source: `packages/contracts/src/chatAttachment.ts:15`.

```text
10 * 1024 * 1024
```

### PROVIDER_SEND_TURN_MAX_IMAGE_DATA_URL_CHARS

Source: `packages/contracts/src/chatAttachment.ts:31`.

```text
14_000_000
```

### PROVIDER_SEND_TURN_MAX_TOTAL_IMAGE_BYTES

Source: `packages/contracts/src/chatAttachment.ts:16`.

```text
80 * 1024 * 1024
```

### PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES

Source: `packages/contracts/src/chatAttachment.ts:18`.

```text
[
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
]
```

### PROVIDER_SLUG_MAX_CHARS

Source: `packages/contracts/src/providerInstance.ts:5`.

```text
64
```

### ParsedJson

Source: `packages/contracts/src/orchestrationV2IdTable.ts:86`.

```text
arbitrary JSON value (no wrapper).
```

### PastedTextAttachmentSource

Source: `packages/contracts/src/chatAttachment.ts:158`.

```text
TaggedObject("pasted-text", {})
```

### PendingBackgroundTaskFields

Source: `packages/contracts/src/orchestrationV2.ts:911`.

```text
{
  taskId: TrimmedNonEmptyString,
  description: Optional(TrimmedNonEmptyString),
  parentTaskId: Optional(TrimmedNonEmptyString),
  awaited: Optional(Const(true)),
}
```

### PersistChatAttachmentsError

Source: `packages/contracts/src/chatAttachment.ts:242`.

```text
Object({_tag: Const("PersistChatAttachmentsError"),
    message: String,
    cause: Optional(Defect()),
  })
```

### PersistChatAttachmentsInput

Source: `packages/contracts/src/chatAttachment.ts:230`.

```text
Object({
  threadId: ThreadId,
  messageId: MessageId,
  attachments: Array(UploadChatAttachment),
})
```

### PersistChatAttachmentsResult

Source: `packages/contracts/src/chatAttachment.ts:237`.

```text
Object({
  attachments: Array(ChatAttachment),
})
```

### PositiveInt

Source: `packages/contracts/src/baseSchemas.ts:32`.

```text
Int.check(isGreaterThanOrEqualTo(1))
```

### PreviewAdjustInput

Source: `packages/contracts/src/preview.ts:211`.

```text
Object({
  threadId: ThreadId,
  tabId: PreviewTabId,
  colorScheme: Optional(PreviewAppearancePreference),
  zoomFactor: Optional(PreviewZoomFactor),
  hardReload: Optional(Boolean),
  clear: Optional(Enum(["cookies", "cache"])),
})
```

### PreviewAnnotationContextRecord

Source: `packages/contracts/src/composerContext.ts:147`.

```text
Object({
  ...recordBase,
  kind: Const("preview-annotation"),
  annotationId: ShortString,
  pageUrl: ShortString,
  pageTitle: NullableShortString,
  comment: BoundedString(COMPOSER_CONTEXT_PREVIEW_COMMENT_MAX_CHARS),
  targetSummary: ShortString,
  styleChanges: Array(ShortString).check(isMaxLength(200)),
  elements: Optional(Array(ElementContextDetails).check(isMaxLength(50))),
  elementIds: Optional(Array(ShortString).check(isMaxLength(50))),
  regionCount: Optional(NonNegativeInt),
  strokeCount: Optional(NonNegativeInt),
  styleChangeDetails: Optional(
    Array(
      Object({
        targetId: ShortString,
        selector: NullableShortString,
        property: ShortString,
        previousValue: BoundedString(COMPOSER_CONTEXT_ELEMENT_STYLES_MAX_CHARS),
        value: BoundedString(COMPOSER_CONTEXT_ELEMENT_STYLES_MAX_CHARS),
      }).check(isMaxLength(200)),
  ),
  screenshotContextId: Optional(ComposerContextId),
})
```

### PreviewAppearancePreference

Source: `packages/contracts/src/preview.ts:113`.

```text
Enum(["system", "light", "dark"])
```

### PreviewClearProfileError

Source: `packages/contracts/src/preview.ts:373`.

```text
Object({_tag: Const("PreviewClearProfileError"), profileId: String, cause: Defect() })
```

### PreviewClearProfileInput

Source: `packages/contracts/src/preview.ts:240`.

```text
Object({
  profileId: BrowserProfileId,
})
```

### PreviewCloseInput

Source: `packages/contracts/src/preview.ts:221`.

```text
Object({
  threadId: ThreadId,
  tabId: Optional(PreviewTabId),
})
```

### PreviewClosedEvent

Source: `packages/contracts/src/preview.ts:301`.

```text
Object({
  ...PreviewEventBasefields,
  type: Const("closed"),
})
```

### PreviewControlRequiredError

Source: `packages/contracts/src/preview.ts:364`.

```text
Object({_tag: Const("PreviewControlRequiredError"), tabId: String })
```

### PreviewError

Source: `packages/contracts/src/preview.ts:382`.

```text
Union([
  PreviewSessionLookupError,
  PreviewInvalidUrlError,
  PreviewControlRequiredError,
])
```

### PreviewEvent

Source: `packages/contracts/src/preview.ts:306`.

```text
Union([
  PreviewOpenedEvent,
  PreviewNavigatedEvent,
  PreviewResizedEvent,
  PreviewFailedEvent,
  PreviewClosedEvent,
])
```

### PreviewEventBaseSchema

Source: `packages/contracts/src/preview.ts:258`.

```text
Object({
  threadId: TrimmedNonEmptyString,
  tabId: PreviewTabId,
  createdAt: String,
  serverEpoch: TrimmedNonEmptyString,
  revision: PositiveInt,
})
```

### PreviewFailedEvent

Source: `packages/contracts/src/preview.ts:291`.

```text
Object({
  ...PreviewEventBasefields,
  type: Const("failed"),
  url: Url,
  title: Title,
  code: Int,
  description: String,
  download: Optional(PreviewFileDownload),
})
```

### PreviewFileDownload

Source: `packages/contracts/src/preview.ts:118`.

```text
Object({ id: String, fileName: String })
```

### PreviewInvalidUrlError

Source: `packages/contracts/src/preview.ts:349`.

```text
Object({_tag: Const("PreviewInvalidUrlError"),
    inputLength: Number,
    reason: Enum(["empty", "parse", "unsupported-protocol", "unexpected"]),
    protocol: Optional(String),
    cause: Defect(),
  })
```

### PreviewListInput

Source: `packages/contracts/src/preview.ts:245`.

```text
Object({
  threadId: ThreadId,
})
```

### PreviewListResult

Source: `packages/contracts/src/preview.ts:250`.

```text
Object({
  sessions: Array(PreviewSessionSnapshot),
  serverEpoch: TrimmedNonEmptyString,
  revision: NonNegativeInt,
})
```

### PreviewNavStatus

Source: `packages/contracts/src/preview.ts:120`.

```text
Union([
  TaggedObject("Idle", {}),
  TaggedObject("Loading", {
    url: Url,
    title: Title,
  }),
  TaggedObject("Success", {
    url: Url,
    title: Title,
  }),
  TaggedObject("LoadFailed", {
    url: Url,
    title: Title,
    code: Int,
    description: String,
    download: Optional(PreviewFileDownload),
  }),
])
```

### PreviewNavigateInput

Source: `packages/contracts/src/preview.ts:180`.

```text
Object({
  threadId: ThreadId,
  tabId: PreviewTabId,
  url: Url,
  resolvedTitle: Optional(Title),
})
```

### PreviewNavigatedEvent

Source: `packages/contracts/src/preview.ts:273`.

```text
Object({
  ...PreviewEventBasefields,
  type: Const("navigated"),
  snapshot: PreviewSessionSnapshot,
})
```

### PreviewOpenInput

Source: `packages/contracts/src/preview.ts:170`.

```text
Object({
  threadId: ThreadId,
  url: Optional(Url),
  viewport: Optional(PreviewViewportSetting),
  profileId: Optional(BrowserProfileId),
  runtime: Optional(PreviewRuntime),
  reveal: Optional(Boolean),
})
```

### PreviewOpenedEvent

Source: `packages/contracts/src/preview.ts:267`.

```text
Object({
  ...PreviewEventBasefields,
  type: Const("opened"),
  snapshot: PreviewSessionSnapshot,
})
```

### PreviewRefreshInput

Source: `packages/contracts/src/preview.ts:197`.

```text
Object({
  threadId: ThreadId,
  tabId: PreviewTabId,
})
```

### PreviewReportProfilesInput

Source: `packages/contracts/src/preview.ts:232`.

```text
Object({
  profiles: Array(BrowserProfile).check(isMaxLength(BROWSER_PROFILE_MAX_COUNT)),
  defaultProfileId: BrowserProfileId,
  recordingFrameRate: Optional(BrowserRecordingFrameRate),
})
```

### PreviewReportStatusInput

Source: `packages/contracts/src/preview.ts:188`.

```text
Object({
  threadId: ThreadId,
  tabId: PreviewTabId,
  navStatus: PreviewNavStatus,
  canGoBack: Boolean,
  canGoForward: Boolean,
})
```

### PreviewResizeInput

Source: `packages/contracts/src/preview.ts:203`.

```text
Object({
  threadId: ThreadId,
  tabId: PreviewTabId,
  viewport: PreviewViewportSetting,
})
```

### PreviewResizedEvent

Source: `packages/contracts/src/preview.ts:279`.

```text
Object({
  ...PreviewEventBasefields,
  type: Const("resized"),
  snapshot: PreviewSessionSnapshot,
  request: Optional(
    Object({
      hardReload: Optional(Boolean),
      clear: Optional(Enum(["cookies", "cache"])),
    }),
})
```

### PreviewRuntime

Source: `packages/contracts/src/preview.ts:140`.

```text
Enum(["desktop", "server"])
```

### PreviewSessionLookupError

Source: `packages/contracts/src/preview.ts:337`.

```text
Object({_tag: Const("PreviewSessionLookupError"),
    threadId: String,
    tabId: String,
  })
```

### PreviewSessionSnapshot

Source: `packages/contracts/src/preview.ts:151`.

```text
Object({
  threadId: TrimmedNonEmptyString,
  tabId: PreviewTabId,
  navStatus: PreviewNavStatus,
  canGoBack: Boolean,
  canGoForward: Boolean,
  viewport: Optional(PreviewViewportSetting),
  colorScheme: Optional(PreviewAppearancePreference),
  zoomFactor: Optional(PreviewZoomFactor),
  profileId: Optional(BrowserProfileId),
  runtime: Optional(PreviewRuntime),
  automationOwner: Optional(String),
  reveal: Optional(Boolean),
  revealRequest: Optional(Object({ id: String, force: Boolean }),
  updatedAt: String,
})
```

### PreviewTabId

Source: `packages/contracts/src/preview.ts:15`.

```text
TrimmedNonEmptyString.check(isMaxLength(128))
```

### PreviewViewportDimension

Source: `packages/contracts/src/preview.ts:22`.

```text
Int.check(
  isBetween({
    minimum: PREVIEW_VIEWPORT_MIN_DIMENSION,
    maximum: PREVIEW_VIEWPORT_MAX_DIMENSION,
  })
```

### PreviewViewportSetting

Source: `packages/contracts/src/preview.ts:87`.

```text
Union([
  TaggedObject("fill", {}),
  TaggedObject("freeform", {
    ...PreviewViewportSize.fields,
  }).check(viewportAreaFilter),
  TaggedObject("preset", {
    ...PreviewViewportSize.fields,
    presetId: StoredPreviewViewportPresetId,
  }).check(viewportAreaFilter),
])
```

### PreviewViewportSize

Source: `packages/contracts/src/preview.ts:35`.

```text
Object({
  width: PreviewViewportDimension,
  height: PreviewViewportDimension,
}).check(viewportAreaFilter)
```

### PreviewZoomFactor

Source: `packages/contracts/src/preview.ts:108`.

```text
Enum(PREVIEW_ZOOM_LEVELS)
```

### Project

Source: `packages/contracts/src/project.ts:151`.

```text
Object({
  id: ProjectId,
  title: TrimmedNonEmptyString,
  workspaceRoot: TrimmedNonEmptyString,
  repositoryIdentity: Optional(Nullable(RepositoryIdentity)),
  faviconPath: Optional(Nullable(TrimmedNonEmptyString)),
  projectIcon: Optional(Nullable(ReceivedProjectIcon)),
  defaultModelSelection: Nullable(ModelSelection),
  defaultThreadEnvMode: Optional(Nullable(ThreadEnvMode)),
  autoPull: Optional(Boolean),
  scripts: Array(ProjectScript),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  deletedAt: Nullable(IsoDateTime),
})
```

### ProjectCloneActionInput

Source: `packages/contracts/src/projectClone.ts:69`.

```text
Object({
  projectId: ProjectId,
})
```

### ProjectCloneActionResult

Source: `packages/contracts/src/projectClone.ts:74`.

```text
Object({
  applied: Boolean,
})
```

### ProjectCloneListEvent

Source: `packages/contracts/src/projectClone.ts:46`.

```text
Array(ProjectCloneSnapshot)
```

### ProjectClonePhase

Source: `packages/contracts/src/projectClone.ts:22`.

```text
Enum(["running", "done", "failed", "cancelled"])
```

### ProjectCloneSnapshot

Source: `packages/contracts/src/projectClone.ts:25`.

```text
Object({
  projectId: ProjectId,
  remoteUrl: TrimmedNonEmptyString,
  destinationPath: TrimmedNonEmptyString,
  repository: Nullable(SourceControlRepositoryInfo),
  phase: ProjectClonePhase,
  stage: ProjectCloneStage,
  percent: Nullable(Int.check(isBetween({ minimum: 0, maximum: 100 })),
  detail: Nullable(String.check(isMaxLength(PROJECT_CLONE_DETAIL_MAX_LENGTH))),
  stalled: Optional(Boolean),
  error: Nullable(String.check(isMaxLength(PROJECT_CLONE_ERROR_MAX_LENGTH))),
  startedAt: IsoDateTime,
  endedAt: Nullable(IsoDateTime),
  sequence: NonNegativeInt,
})
```

### ProjectCloneStage

Source: `packages/contracts/src/projectClone.ts:13`.

```text
Enum([
  "connecting",
  "counting",
  "receiving",
  "resolving",
  "checkout",
])
```

### ProjectCloneStartInput

Source: `packages/contracts/src/projectClone.ts:49`.

```text
Object({
  projectId: ProjectId,
  title: TrimmedNonEmptyString,
  createdAt: IsoDateTime,
  provider: Optional(SourceControlProviderKind),
  repository: Optional(TrimmedNonEmptyString),
  remoteUrl: Optional(TrimmedNonEmptyString),
  destinationPath: TrimmedNonEmptyString,
  protocol: Optional(SourceControlCloneProtocol),
})
```

### ProjectCloneStartResult

Source: `packages/contracts/src/projectClone.ts:61`.

```text
Object({
  projectId: ProjectId,
  cwd: TrimmedNonEmptyString,
  remoteUrl: TrimmedNonEmptyString,
  repository: Nullable(SourceControlRepositoryInfo),
})
```

### ProjectCloneSubscribeInput

Source: `packages/contracts/src/projectClone.ts:43`.

```text
Object({})
```

### ProjectContentMatch

Source: `packages/contracts/src/project.ts:279`.

```text
Object({
  path: TrimmedNonEmptyString,
  lineNumber: PositiveInt,
  lineContent: String,
  matchRanges: Array(ProjectContentMatchRange),
})
```

### ProjectContentMatchRange

Source: `packages/contracts/src/project.ts:273`.

```text
Object({
  start: NonNegativeInt,
  end: NonNegativeInt,
})
```

### ProjectCreateNewInput

Source: `packages/contracts/src/project.ts:527`.

```text
Object({
  name: TrimmedNonEmptyString.check(isMaxLength(200)),
})
```

### ProjectCreateNewResult

Source: `packages/contracts/src/project.ts:532`.

```text
Object({
  projectId: ProjectId,
  workspaceRoot: TrimmedNonEmptyString,
  commitError: Optional(TrimmedNonEmptyString),
})
```

### ProjectCreatePayload

Source: `packages/contracts/src/project.ts:185`.

```text
Object({
  title: TrimmedNonEmptyString,
  workspaceRoot: TrimmedNonEmptyString,
  createWorkspaceRootIfMissing: Optional(Boolean),
  defaultModelSelection: Optional(Nullable(ModelSelection)),
  scripts: Optional(Array(ProjectScript)),
})
```

### ProjectEmoji

Source: `packages/contracts/src/project.ts:73`.

```text
TrimmedNonEmptyString.check(isMaxLength(32))
```

### ProjectEmojiIcon

Source: `packages/contracts/src/project.ts:86`.

```text
Object({
  kind: Const("emoji"),
  emoji: ProjectEmoji,
})
```

### ProjectEnsureScratchResult

Source: `packages/contracts/src/project.ts:522`.

```text
Object({
  projectId: ProjectId,
})
```

### ProjectEntriesFailure

Source: `packages/contracts/src/project.ts:325`.

```text
Enum([
  "workspace_root_not_found",
  "workspace_root_create_failed",
  "workspace_root_stat_failed",
  "workspace_root_not_directory",
  "search_index_create_failed",
  "search_index_scan_timed_out",
  "search_index_search_failed",
  "directory_list_failed",
])
```

### ProjectEntry

Source: `packages/contracts/src/project.ts:249`.

```text
Object({
  path: TrimmedNonEmptyString,
  kind: ProjectEntryKind,
  ignored: Optional(Boolean),
})
```

### ProjectEntryKind

Source: `packages/contracts/src/project.ts:237`.

```text
Enum(["file", "directory"])
```

### ProjectFaviconPath

Source: `packages/contracts/src/project.ts:95`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(1024),
  isPattern(/\.(?:avif|gif|icns|ico|jpe?g|png|svg|webp)$/i),
)
```

### ProjectFileFailure

Source: `packages/contracts/src/project.ts:453`.

```text
Enum([
  "workspace_path_outside_root",
  "resolved_path_outside_root",
  "path_not_file",
  "binary_file",
  "operation_failed",
])
```

### ProjectFileOperation

Source: `packages/contracts/src/project.ts:462`.

```text
Enum([
  "realpath-workspace-root",
  "realpath-target",
  "open",
  "stat",
  "read",
  "close",
  "make-directory",
  "write-file",
])
```

### ProjectIconColor

Source: `packages/contracts/src/project.ts:46`.

```text
Enum([
  "gray",
  "red",
  "orange",
  "amber",
  "yellow",
  "lime",
  "green",
  "emerald",
  "teal",
  "cyan",
  "sky",
  "blue",
  "indigo",
  "violet",
  "purple",
  "fuchsia",
  "pink",
  "rose",
])
```

### ProjectIconOverride

Source: `packages/contracts/src/project.ts:101`.

```text
Union([
  ProjectLucideIcon,
  ProjectEmojiIcon,
  ProjectMonogramIcon,
])
```

### ProjectIgnoredFile

Source: `packages/contracts/src/project.ts:313`.

```text
Object({
  path: TrimmedNonEmptyString,
  byteLength: NonNegativeInt,
})
```

### ProjectListEntriesError

Source: `packages/contracts/src/project.ts:412`.

```text
Object({_tag: Const("ProjectListEntriesError"),
    cwd: Optional(TrimmedNonEmptyString),
    failure: Optional(ProjectEntriesFailure),
    normalizedCwd: Optional(TrimmedNonEmptyString),
    timeout: Optional(TrimmedNonEmptyString),
    detail: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ProjectListEntriesInput

Source: `packages/contracts/src/project.ts:294`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  directoryPath: Optional(TrimmedString),
})
```

### ProjectListEntriesResult

Source: `packages/contracts/src/project.ts:300`.

```text
Object({
  entries: Array(ProjectEntry),
  truncated: Boolean,
})
```

### ProjectListIgnoredFilesInput

Source: `packages/contracts/src/project.ts:308`.

```text
Object({
  cwd: TrimmedNonEmptyString,
})
```

### ProjectListIgnoredFilesResult

Source: `packages/contracts/src/project.ts:319`.

```text
Object({
  files: Array(ProjectIgnoredFile),
  truncated: Boolean,
})
```

### ProjectLucideIcon

Source: `packages/contracts/src/project.ts:81`.

```text
Object({
  kind: Const("lucide"),
  name: ProjectLucideIconName,
  color: ProjectIconColor,
})
```

### ProjectLucideIconName

Source: `packages/contracts/src/project.ts:68`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(64),
  isPattern(/^[a-z0-9]+(?:-[a-z0-9]+)*$/),
)
```

### ProjectLucideIconWithLegacyMonogram

Source: `packages/contracts/src/project.ts:108`.

```text
Object({
  ...ProjectLucideIcon.fields,
  monogramText: Optional(ProjectMonogramText),
  monogram: Optional(ProjectMonogramText),
})
```

### ProjectMonogramIcon

Source: `packages/contracts/src/project.ts:90`.

```text
Object({
  kind: Const("monogram"),
  text: ProjectMonogramText,
  color: ProjectIconColor,
})
```

### ProjectMonogramText

Source: `packages/contracts/src/project.ts:76`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(32),
  isPattern(/^[\p{L}\p{N}][\p{L}\p{N}\p{M}\u200c\u200d]*$/u),
)
```

### ProjectMutation

Source: `packages/contracts/src/project.ts:206`.

```text
Union([
  Object({
    type: Const("project.create"),
    commandId: CommandId,
    projectId: ProjectId,
    ...ProjectCreatePayload.fields,
  }),
  Object({
    type: Const("project.update"),
    commandId: CommandId,
    projectId: ProjectId,
    ...ProjectUpdatePayload.fields,
  }),
  Object({
    type: Const("project.delete"),
    commandId: CommandId,
    projectId: ProjectId,
    force: Optional(Boolean),
  }),
])
```

### ProjectMutationError

Source: `packages/contracts/src/project.ts:228`.

```text
Object({_tag: Const("ProjectMutationError"),
    commandId: CommandId,
    message: String,
    cause: Optional(Defect()),
  })
```

### ProjectProviderInstanceOverride

Source: `packages/contracts/src/settings.ts:985`.

```text
Object({
  enabled: Optional(Boolean),
})
```

### ProjectReadFileError

Source: `packages/contracts/src/project.ts:485`.

```text
Object({_tag: Const("ProjectReadFileError"),
    cwd: Optional(TrimmedNonEmptyString),
    relativePath: Optional(TrimmedNonEmptyString),
    failure: Optional(ProjectFileFailure),
    resolvedPath: Optional(TrimmedNonEmptyString),
    resolvedWorkspaceRoot: Optional(TrimmedNonEmptyString),
    operation: Optional(ProjectFileOperation),
    operationPath: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ProjectReadFileInput

Source: `packages/contracts/src/project.ts:434`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  relativePath: TrimmedNonEmptyString.check(isMaxLength(PROJECT_READ_FILE_PATH_MAX_LENGTH)),
  allowMissing: Optional(Boolean),
})
```

### ProjectReadFileResult

Source: `packages/contracts/src/project.ts:443`.

```text
Object({
  relativePath: TrimmedNonEmptyString,
  contents: String,
  byteLength: NonNegativeInt,
  truncated: Boolean,
  exists: Optional(Boolean),
})
```

### ProjectScript

Source: `packages/contracts/src/project.ts:33`.

```text
Object({
  id: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString,
  command: TrimmedNonEmptyString,
  icon: ProjectScriptIcon,
  runOnWorktreeCreate: Boolean,
  runOnSettle: Optional(Boolean),
  async: Optional(Boolean),
  previewUrl: Optional(TrimmedNonEmptyString),
  autoOpenPreview: Optional(Boolean),
})
```

### ProjectScriptIcon

Source: `packages/contracts/src/project.ts:23`.

```text
Enum([
  "play",
  "test",
  "lint",
  "configure",
  "build",
  "debug",
])
```

### ProjectSearchContentsError

Source: `packages/contracts/src/project.ts:381`.

```text
Object({_tag: Const("ProjectSearchContentsError"),
    cwd: Optional(TrimmedNonEmptyString),
    queryLength: Optional(NonNegativeInt),
    limit: Optional(PositiveInt),
    failure: Optional(ProjectEntriesFailure),
    normalizedCwd: Optional(TrimmedNonEmptyString),
    timeout: Optional(TrimmedNonEmptyString),
    detail: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ProjectSearchContentsInput

Source: `packages/contracts/src/project.ts:262`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  query: String.check(isNonEmpty(), isMaxLength(256)),
  limit: PositiveInt.check(isLessThanOrEqualTo(PROJECT_SEARCH_CONTENTS_MAX_LIMIT)),
  caseSensitive: Boolean,
  wholeWord: Boolean,
  useRegex: Boolean,
})
```

### ProjectSearchContentsResult

Source: `packages/contracts/src/project.ts:287`.

```text
Object({
  matches: Array(ProjectContentMatch),
  truncated: Boolean,
  regexFallbackError: Optional(String),
})
```

### ProjectSearchEntriesError

Source: `packages/contracts/src/project.ts:350`.

```text
Object({_tag: Const("ProjectSearchEntriesError"),
    cwd: Optional(TrimmedNonEmptyString),
    queryLength: Optional(NonNegativeInt),
    limit: Optional(PositiveInt),
    failure: Optional(ProjectEntriesFailure),
    normalizedCwd: Optional(TrimmedNonEmptyString),
    timeout: Optional(TrimmedNonEmptyString),
    detail: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ProjectSearchEntriesInput

Source: `packages/contracts/src/project.ts:240`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  query: TrimmedString.check(isMaxLength(256)),
  limit: PositiveInt.check(isLessThanOrEqualTo(PROJECT_SEARCH_ENTRIES_MAX_LIMIT)),
  kind: Optional(ProjectEntryKind),
  imageOnly: Optional(Boolean),
})
```

### ProjectSearchEntriesResult

Source: `packages/contracts/src/project.ts:256`.

```text
Object({
  entries: Array(ProjectEntry),
  truncated: Boolean,
})
```

### ProjectSettingsOverrides

Source: `packages/contracts/src/settings.ts:992`.

```text
Object({
  worktreeCleanup: Optional(WorktreeCleanup),
  defaultModelSelection: Optional(Nullable(ModelSelection)),
  defaultRuntimeMode: Optional(RuntimeMode),
  defaultThreadEnvMode: Optional(ThreadEnvMode),
  newWorktreesStartFromOrigin: Optional(Boolean),
  worktreeSubmodules: ForwardCompatibleOptional(WorktreeSubmodules),
  defaultAutoPull: Optional(Boolean),
  defaultProjectScripts: Optional(Array(ProjectScript)),
  enableAgentBrowserAccess: Optional(Boolean),
  enableAgentDeviceAccess: Optional(Boolean),
  textGenerationModelSelection: Optional(ModelSelection),
  threadTitleInstructions: Optional(TrimmedString),
  sourceControlWriterModelSelection: Optional(Nullable(ModelSelection)),
  reviewGuideModelSelection: Optional(Nullable(ModelSelection)),
  reviewGuideInstructions: Optional(TrimmedString),
  sourceControlWritingStyle: Optional(SourceControlWritingStyleSetting),
  removeAgentCreditsOnMerge: Optional(Boolean),
  branchNamingMode: Optional(BranchNamingMode),
  branchNamePrefix: Optional(TrimmedString),
  branchNameInstructions: Optional(TrimmedString),
  pullRequestMergeMethod: Optional(Nullable(PullRequestMergeMethod)),
  sidebarAutoSettleOnMerge: Optional(Boolean),
  sidebarAutoSettleAfterDays: Optional(Nullable(SidebarAutoSettleAfterDays)),
  sidebarAutoSettleKeepPullRequests: Optional(Boolean),
  continueThreadsAfterServerUpdate: Optional(Boolean),
  responseStreamingMode: Optional(ResponseStreamingMode),
  providerInstances: Optional(
    Record(ProviderInstanceId, ProjectProviderInstanceOverride),
  ),
} satisfies Record<ProjectScopedServerSettingKey | "providerInstances", unknown>)
```

### ProjectSnapshot

Source: `packages/contracts/src/project.ts:169`.

```text
Object({
  projects: Array(Project),
  updatedAt: IsoDateTime,
})
```

### ProjectUpdatePayload

Source: `packages/contracts/src/project.ts:194`.

```text
Object({
  title: Optional(TrimmedNonEmptyString),
  workspaceRoot: Optional(TrimmedNonEmptyString),
  defaultModelSelection: Optional(Nullable(ModelSelection)),
  autoPull: Optional(Boolean),
  projectIcon: Optional(Nullable(ProjectIconOverride)),
  faviconPath: Optional(Nullable(TrimmedNonEmptyString)),
  defaultThreadEnvMode: Optional(Nullable(ThreadEnvMode)),
  scripts: Optional(Array(ProjectScript)),
})
```

### ProjectWriteFileError

Source: `packages/contracts/src/project.ts:540`.

```text
Object({_tag: Const("ProjectWriteFileError"),
    cwd: Optional(TrimmedNonEmptyString),
    relativePath: Optional(TrimmedNonEmptyString),
    failure: Optional(ProjectFileFailure),
    resolvedPath: Optional(TrimmedNonEmptyString),
    resolvedWorkspaceRoot: Optional(TrimmedNonEmptyString),
    operation: Optional(ProjectFileOperation),
    operationPath: Optional(TrimmedNonEmptyString),
    message: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ProjectWriteFileInput

Source: `packages/contracts/src/project.ts:510`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  relativePath: TrimmedNonEmptyString.check(isMaxLength(PROJECT_WRITE_FILE_PATH_MAX_LENGTH)),
  contents: String,
})
```

### ProjectWriteFileResult

Source: `packages/contracts/src/project.ts:517`.

```text
Object({
  relativePath: TrimmedNonEmptyString,
})
```

### ProviderApprovalDecision

Source: `packages/contracts/src/providerPolicy.ts:50`.

```text
Enum([
  "accept",
  "acceptForSession",
  "acceptAlways",
  "decline",
  "cancel",
])
```

### ProviderApprovalFullAccessOverride

Source: `packages/contracts/src/providerPolicy.ts:61`.

```text
Enum(["ask_rule", "provider_check"])
```

### ProviderApprovalOption

Source: `packages/contracts/src/providerPolicy.ts:64`.

```text
Object({
  decision: ProviderApprovalDecision,
  label: TrimmedNonEmptyString,
  warning: Optional(TrimmedNonEmptyString),
})
```

### ProviderAuthCancelInput

Source: `packages/contracts/src/providerSetup.ts:149`.

```text
Object({
  instanceId: ProviderInstanceId,
  flowId: SetupOperationId,
})
```

### ProviderAuthCompleteInput

Source: `packages/contracts/src/providerSetup.ts:142`.

```text
Object({
  instanceId: ProviderInstanceId,
  flowId: SetupOperationId,
  callbackUrl: TrimmedNonEmptyString.check(isMaxLength(16_384)),
})
```

### ProviderAuthInteraction

Source: `packages/contracts/src/providerSetup.ts:44`.

```text
Union([
  Object({
    type: Const("browser"),
    id: SetupOperationId,
    url: TrimmedNonEmptyString.check(isMaxLength(16_384)),
    requiresConsent: Boolean,
    acceptsCallback: Optional(Boolean),
  }),
  Object({
    type: Const("deviceCode"),
    id: SetupOperationId,
    url: TrimmedNonEmptyString.check(isMaxLength(16_384)),
    userCode: TrimmedNonEmptyString.check(isMaxLength(256)),
  }),
  Object({
    type: Const("terminal"),
    id: SetupOperationId,
    output: String.check(isMaxLength(16_384)),
    outputOffset: Optional(Int.check(isGreaterThanOrEqualTo(0))),
  }),
  Object({
    type: Const("credentials"),
    id: SetupOperationId,
    fields: Array(
      Object({
        name: SetupOperationId,
        label: TrimmedNonEmptyString,
        secret: Boolean,
      }).check(isMaxLength(16)),
  }),
])
```

### ProviderAuthMethod

Source: `packages/contracts/src/providerSetup.ts:35`.

```text
Object({
  id: SetupOperationId,
  accountEmail: Optional(TrimmedNonEmptyString),
  name: TrimmedNonEmptyString,
  description: Nullable(String),
  type: Enum(["agent", "terminal", "credentials"]),
})
```

### ProviderAuthRespondInput

Source: `packages/contracts/src/providerSetup.ts:110`.

```text
Object({
  instanceId: ProviderInstanceId,
  flowId: SetupOperationId,
  interactionId: SetupOperationId,
  response: ProviderAuthResponse,
})
```

### ProviderAuthResponse

Source: `packages/contracts/src/providerSetup.ts:78`.

```text
Union([
  Object({
    type: Const("browser"),
    action: Enum(["accept", "decline"]),
  }),
  Object({
    type: Const("terminal"),
    data: String.check(isMaxLength(4_096)),
    size: Optional(
      Object({
        cols: Int.check(isBetween({ minimum: 1, maximum: 500 }),
        rows: Int.check(isBetween({ minimum: 1, maximum: 200 }),
      }),
  }),
  Object({
    type: Const("credentials"),
    values: Record(SetupOperationId, String.check(isMaxLength(16_384))).check(
      isMaxProperties(16),
    ),
  }),
])
```

### ProviderAuthStartInput

Source: `packages/contracts/src/providerSetup.ts:102`.

```text
Object({
  instanceId: ProviderInstanceId,
  methodId: Optional(SetupOperationId),
  returnUrl: Optional(String),
  callbackMode: Optional(Enum(["server", "client"])),
})
```

### ProviderAuthState

Source: `packages/contracts/src/providerSetup.ts:118`.

```text
Object({
  instanceId: ProviderInstanceId,
  phase: Enum([
    "idle",
    "starting",
    "waiting",
    "verifying",
    "succeeded",
    "failed",
    "cancelled",
  ]),
  flowId: Nullable(SetupOperationId),
  authorizationUrl: Nullable(String),
  expiresAt: Nullable(IsoDateTime),
  message: Nullable(String),
  methods: Optional(
    ForwardCompatibleArray(ProviderAuthMethod).check(isMaxLength(32)),
  ),
  interaction: ForwardCompatibleOptional(Nullable(ProviderAuthInteraction)),
  credentialOwner: ForwardCompatibleOptional(Enum(["provider", "t3"])),
})
```

### ProviderConsumeResetCreditInput

Source: `packages/contracts/src/providerUsageLimits.ts:89`.

```text
Union([
  Object({ instanceId: ProviderInstanceId }),
  UsageLimitSourceConsumeResetCreditInput,
])
```

### ProviderConsumeResetCreditOutcome

Source: `packages/contracts/src/providerUsageLimits.ts:104`.

```text
Enum([
  "reset",
  "nothingToReset",
  "noCredit",
  "alreadyRedeemed",
])
```

### ProviderConsumeResetCreditResult

Source: `packages/contracts/src/providerUsageLimits.ts:112`.

```text
Object({
  outcome: ProviderConsumeResetCreditOutcome,
  warning: Optional(TrimmedNonEmptyString),
})
```

### ProviderDriverKind

Source: `packages/contracts/src/providerInstance.ts:15`.

```text
slugSchema
```

### ProviderInstallCancelInput

Source: `packages/contracts/src/providerSetup.ts:180`.

```text
Object({
  instanceId: ProviderInstanceId,
  operationId: SetupOperationId,
})
```

### ProviderInstallState

Source: `packages/contracts/src/providerSetup.ts:157`.

```text
Object({
  driver: ProviderDriverKind,
  operationId: Nullable(SetupOperationId),
  phase: Enum([
    "idle",
    "downloading",
    "extracting",
    "verifying",
    "succeeded",
    "failed",
    "cancelled",
  ]),
  downloadedBytes: ByteCount,
  totalBytes: Nullable(ByteCount),
  version: Nullable(TrimmedNonEmptyString),
  installedVersion: Nullable(TrimmedNonEmptyString),
  executablePath: Optional(Nullable(TrimmedNonEmptyString)),
  source: Optional(Nullable(Enum(["managed", "local"]))),
  canRemove: Boolean,
  message: Nullable(String),
})
```

### ProviderInstanceConfig

Source: `packages/contracts/src/providerInstance.ts:50`.

```text
Object({
  driver: ProviderDriverKind,
  displayName: Optional(TrimmedNonEmptyString),
  accentColor: Optional(TrimmedNonEmptyString),
  environment: Optional(ProviderInstanceEnvironment),
  enabled: Optional(Boolean),
  config: Optional(Unknown),
})
```

### ProviderInstanceEnvironment

Source: `packages/contracts/src/providerInstance.ts:46`.

```text
Array(ProviderInstanceEnvironmentVariable)
```

### ProviderInstanceEnvironmentVariable

Source: `packages/contracts/src/providerInstance.ts:38`.

```text
Object({
  name: ProviderInstanceEnvironmentVariableName,
  value: String,
  sensitive: Boolean,
  valueRedacted: Optional(Boolean),
})
```

### ProviderInstanceEnvironmentVariableName

Source: `packages/contracts/src/providerInstance.ts:31`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(ENVIRONMENT_VARIABLE_NAME_MAX_CHARS),
  isPattern(ENVIRONMENT_VARIABLE_NAME_PATTERN),
)
```

### ProviderInstanceId

Source: `packages/contracts/src/providerInstance.ts:22`.

```text
slugSchema
```

### ProviderInstanceMutation

Source: `packages/contracts/src/providerInstance.ts:60`.

```text
Union([
  Object({
    operation: Const("create"),
    instanceId: ProviderInstanceId,
    instance: ProviderInstanceConfig,
  }),
  Object({
    operation: Const("upsert"),
    instanceId: ProviderInstanceId,
    instance: ProviderInstanceConfig,
  }),
  Object({
    operation: Const("remove"),
    instanceId: ProviderInstanceId,
  }),
])
```

### ProviderInteractionMode

Source: `packages/contracts/src/providerPolicy.ts:34`.

```text
Enum(["default", "plan"])
```

### ProviderModelPreferences

Source: `packages/contracts/src/settings.ts:297`.

```text
Record(
  ProviderInstanceId,
  Object({
    hiddenModels: Array(String),
    modelOrder: Array(String),
  })
```

### ProviderOptionChoice

Source: `packages/contracts/src/model.ts:10`.

```text
Object({
  id: TrimmedNonEmptyString,
  label: TrimmedNonEmptyString,
  description: Optional(TrimmedNonEmptyString),
  isDefault: Optional(Boolean),
})
```

### ProviderOptionDescriptor

Source: `packages/contracts/src/model.ts:40`.

```text
Union([
  SelectProviderOptionDescriptor,
  BooleanProviderOptionDescriptor,
])
```

### ProviderOptionDescriptorBase

Source: `packages/contracts/src/model.ts:18`.

```text
{
  id: TrimmedNonEmptyString,
  label: TrimmedNonEmptyString,
  description: Optional(TrimmedNonEmptyString),
}
```

### ProviderOptionSelection

Source: `packages/contracts/src/model.ts:49`.

```text
Object({
  id: TrimmedNonEmptyString,
  value: ProviderOptionSelectionValue,
})
```

### ProviderOptionSelectionValue

Source: `packages/contracts/src/model.ts:46`.

```text
Union([TrimmedNonEmptyString, Boolean])
```

### ProviderOptionSelections

Source: `packages/contracts/src/model.ts:67`.

```text
Union([
  Array(ProviderOptionSelection),
  ProviderOptionSelectionsFromLegacyObject,
])
```

### ProviderOptionSelectionsFromLegacyObject

Source: `packages/contracts/src/model.ts:57`.

```text
LegacyProviderOptionSelectionsObject
```

### ProviderRequestKind

Source: `packages/contracts/src/providerPolicy.ts:38`.

```text
Enum([
  "command",
  "file-read",
  "file-change",
  "mcp-elicitation",
  "permission",
])
```

### ProviderSetupError

Source: `packages/contracts/src/providerSetup.ts:187`.

```text
Object({_tag: Const("ProviderSetupError"),
    instanceId: ProviderInstanceId,
    operation: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### ProviderSetupInput

Source: `packages/contracts/src/providerSetup.ts:12`.

```text
Object({
  instanceId: ProviderInstanceId,
})
```

### ProviderUploadFeedbackError

Source: `packages/contracts/src/provider.ts:128`.

```text
Object({_tag: Const("ProviderUploadFeedbackError"),
    threadId: ThreadId,
    cause: Optional(Defect()),
  })
```

### ProviderUploadFeedbackInput

Source: `packages/contracts/src/provider.ts:117`.

```text
Object({
  threadId: ThreadId,
  reason: Optional(TrimmedNonEmptyString),
})
```

### ProviderUploadFeedbackResult

Source: `packages/contracts/src/provider.ts:123`.

```text
Object({
  feedbackId: TrimmedNonEmptyString,
})
```

### ProviderUserInputAnswers

Source: `packages/contracts/src/providerPolicy.ts:71`.

```text
Record(String, Unknown)
```

### PullRequestAction

Source: `packages/contracts/src/pullRequest.ts:60`.

```text
Enum([
  "merge",
  "ready",
  "draft",
  "close",
  "reopen",
  "delete-branch",
  "update-branch",
  "enable-auto-merge",
  "disable-auto-merge",
  "revert",
  "approve-workflows",
])
```

### PullRequestActionInput

Source: `packages/contracts/src/pullRequest.ts:725`.

```text
Object({
  stackNumber: Optional(PositiveInt),
  expectedStackHeads: Optional(Array(PullRequestStackHead)),
  ...PullRequestRef.fields,
  action: PullRequestAction,
  mergeMethod: Optional(PullRequestMergeMethod),
  updateMethod: Optional(PullRequestUpdateMethod),
})
```

### PullRequestActivity

Source: `packages/contracts/src/pullRequest.ts:602`.

```text
Object({
  author: Optional(Nullable(PullRequestActor)),
  reviewers: Optional(Array(PullRequestActor)),
  comments: Array(PullRequestComment),
  commentCount: NonNegativeInt,
  commentsTruncated: Boolean,
  reviewThreadsTruncated: Optional(Boolean),
  reviewThreads: Array(PullRequestReviewThread),
  commits: Array(PullRequestCommit),
  reactions: Optional(Array(PullRequestReaction)),
})
```

### PullRequestActor

Source: `packages/contracts/src/pullRequest.ts:81`.

```text
Object({
  isBot: Optional(Boolean),
  login: TrimmedNonEmptyString,
  name: Nullable(String),
  avatarUrl: Nullable(String),
})
```

### PullRequestBaseComparison

Source: `packages/contracts/src/pullRequest.ts:78`.

```text
Enum(["up-to-date", "behind", "unknown"])
```

### PullRequestCapabilities

Source: `packages/contracts/src/pullRequest.ts:259`.

```text
Object({
  diff: Boolean,
  comment: Boolean,
  actions: ForwardCompatibleArray(PullRequestAction),
  mergeMethods: ForwardCompatibleArray(PullRequestMergeMethod),
  deleteBranch: Optional(Boolean),
  updateMethods: Optional(ForwardCompatibleArray(PullRequestUpdateMethod)),
  search: Boolean,
  reactions: Optional(Boolean),
  viewedFiles: Optional(PullRequestViewedFilesStore),
  review: PullRequestReviewCapabilities,
  reviewers: PullRequestReviewerCapabilities,
  edit: Optional(PullRequestEditCapabilities),
  stacks: Optional(Boolean),
  stackActions: Optional(Boolean),
  labels: Optional(Boolean),
  checksAlwaysReport: Optional(Boolean),
})
```

### PullRequestChangeBreakdown

Source: `packages/contracts/src/pullRequest.ts:692`.

```text
Nullable(
  Object({
    code: PullRequestLineCount,
    tests: PullRequestLineCount,
    mocks: PullRequestLineCount,
    generated: PullRequestLineCount,
    docs: PullRequestLineCount,
  })
```

### PullRequestCheck

Source: `packages/contracts/src/pullRequest.ts:106`.

```text
Object({
  name: TrimmedNonEmptyString,
  status: PullRequestCheckStatus,
  description: Nullable(String),
  url: Nullable(String),
  required: Optional(Boolean),
})
```

### PullRequestCheckStatus

Source: `packages/contracts/src/pullRequest.ts:95`.

```text
Enum([
  "pending",
  "action-required",
  "success",
  "failure",
  "skipped",
  "neutral",
  "cancelled",
])
```

### PullRequestChecks

Source: `packages/contracts/src/pullRequest.ts:595`.

```text
Object({
  observedAt: Optional(Finite),
  state: PullRequestState,
  checks: Array(PullRequestCheck),
})
```

### PullRequestChecksState

Source: `packages/contracts/src/pullRequest.ts:51`.

```text
Enum(["passing", "failing", "pending"])
```

### PullRequestComment

Source: `packages/contracts/src/pullRequest.ts:143`.

```text
Object({
  id: TrimmedNonEmptyString,
  kind: PullRequestCommentKind,
  author: Nullable(PullRequestActor),
  body: String,
  createdAt: IsoDateTime,
  editedAt: Optional(Nullable(IsoDateTime)),
  url: Nullable(String),
  path: Nullable(String),
  reviewState: Nullable(String),
  reactions: Optional(Array(PullRequestReaction)),
  canEdit: Optional(Boolean),
})
```

### PullRequestCommentInput

Source: `packages/contracts/src/pullRequest.ts:739`.

```text
Object({
  ...PullRequestRef.fields,
  body: CommentBody,
})
```

### PullRequestCommentKind

Source: `packages/contracts/src/pullRequest.ts:136`.

```text
Enum([
  "issue-comment",
  "review-comment",
  "review",
])
```

### PullRequestCommentUpdateInput

Source: `packages/contracts/src/pullRequest.ts:753`.

```text
Object({
  ...PullRequestRef.fields,
  commentId: TrimmedNonEmptyString,
  kind: Enum(["issue-comment", "review-comment"]),
  body: CommentBody,
})
```

### PullRequestCommit

Source: `packages/contracts/src/pullRequest.ts:221`.

```text
Object({
  oid: TrimmedNonEmptyString,
  messageHeadline: String,
  committedDate: IsoDateTime,
  additions: Optional(NonNegativeInt),
  deletions: Optional(NonNegativeInt),
  authors: Optional(Array(PullRequestActor)),
})
```

### PullRequestContextMetadata

Source: `packages/contracts/src/composerContext.ts:70`.

```text
Object({
  number: PositiveInt,
  title: ShortString,
  url: ShortString,
  headBranch: ShortString,
  baseBranch: ShortString,
  state: Enum(["open", "closed", "merged"]),
  isDraft: Boolean,
})
```

### PullRequestDetail

Source: `packages/contracts/src/pullRequest.ts:553`.

```text
Object({
  provider: SourceControlProviderKind,
  capabilities: PullRequestCapabilities,
  viewerPermissions: PullRequestViewerPermissions,
  projectId: ProjectId,
  projectTitle: TrimmedNonEmptyString,
  workspaceRoot: TrimmedNonEmptyString,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  title: TrimmedNonEmptyString,
  body: String,
  url: TrimmedNonEmptyString,
  author: Nullable(PullRequestActor),
  state: PullRequestState,
  isDraft: Boolean,
  mergeability: PullRequestMergeability,
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
  changedFiles: NonNegativeInt,
  headBranch: TrimmedNonEmptyString,
  headRepositoryNameWithOwner: Optional(Nullable(TrimmedNonEmptyString)),
  headSha: Optional(TrimmedNonEmptyString),
  baseBranch: TrimmedNonEmptyString,
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  observedAt: Optional(Finite),
  mergedAt: Nullable(IsoDateTime),
  closedAt: Nullable(IsoDateTime),
  reviewers: Array(PullRequestActor),
  labels: Array(PullRequestLabel),
  checks: Array(PullRequestCheck),
  mergeCapabilities: PullRequestMergeCapabilities,
  viewer: Optional(TrimmedNonEmptyString),
  baseComparison: Optional(PullRequestBaseComparison),
  behindBy: Optional(NonNegativeInt),
  autoMergeEnabled: Optional(Boolean),
  autoMergeMethod: Optional(PullRequestMergeMethod),
  workflowApprovalsRequired: Optional(NonNegativeInt),
})
```

### PullRequestDiffFileContentsInput

Source: `packages/contracts/src/pullRequest.ts:647`.

```text
Object({
  ...PullRequestRef.fields,
  commit: Optional(TrimmedNonEmptyString),
  changeType: Enum(["change", "rename-pure", "rename-changed", "new", "deleted"]),
  oldPath: TrimmedNonEmptyString,
  newPath: TrimmedNonEmptyString,
})
```

### PullRequestDiffFileContentsResult

Source: `packages/contracts/src/pullRequest.ts:656`.

```text
Object({
  oldContents: String,
  newContents: String,
})
```

### PullRequestDiffInput

Source: `packages/contracts/src/pullRequest.ts:624`.

```text
Object({
  ...PullRequestRef.fields,
  cursor: Optional(TrimmedNonEmptyString),
  commit: Optional(TrimmedNonEmptyString),
})
```

### PullRequestDiffResult

Source: `packages/contracts/src/pullRequest.ts:638`.

```text
Object({
  patch: String,
  truncated: Boolean,
  nextCursor: Nullable(TrimmedNonEmptyString),
  omittedFileStats: Optional(Array(PullRequestOmittedFileStat)),
})
```

### PullRequestDiffSide

Source: `packages/contracts/src/pullRequest.ts:158`.

```text
Enum(["left", "right"])
```

### PullRequestDiffStat

Source: `packages/contracts/src/pullRequest.ts:497`.

```text
Object({
  projectId: ProjectId,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
})
```

### PullRequestEditCapabilities

Source: `packages/contracts/src/pullRequest.ts:244`.

```text
Object({
  changeRequest: Boolean,
  comment: Boolean,
})
```

### PullRequestFileViewed

Source: `packages/contracts/src/pullRequest.ts:669`.

```text
Object({
  path: FilePath,
  state: PullRequestFileViewedState,
})
```

### PullRequestFileViewedState

Source: `packages/contracts/src/pullRequest.ts:666`.

```text
Enum(["unviewed", "viewed", "dismissed"])
```

### PullRequestFilesViewedResult

Source: `packages/contracts/src/pullRequest.ts:675`.

```text
Object({
  files: Array(PullRequestFileViewed),
  truncated: Boolean,
})
```

### PullRequestFilterOptionKind

Source: `packages/contracts/src/pullRequest.ts:516`.

```text
Enum(["labels", "authors"])
```

### PullRequestFilterOptions

Source: `packages/contracts/src/pullRequest.ts:529`.

```text
Object({
  labels: Array(PullRequestLabel),
  authors: Array(PullRequestActor),
  truncated: Boolean,
})
```

### PullRequestFilterOptionsInput

Source: `packages/contracts/src/pullRequest.ts:520`.

```text
Object({
  kind: PullRequestFilterOptionKind,
  projectId: Optional(ProjectId),
  projectIds: Optional(Array(ProjectId).check(isMaxLength(100))),
  host: Optional(TrimmedNonEmptyString),
  query: Optional(TrimmedNonEmptyString.check(isMaxLength(100))),
})
```

### PullRequestInvalidateInput

Source: `packages/contracts/src/pullRequest.ts:540`.

```text
Object({
  reference: Optional(PullRequestRef),
  filesViewedOnly: Optional(Boolean),
})
```

### PullRequestInvolvement

Source: `packages/contracts/src/pullRequest.ts:17`.

```text
Enum(["all", "reviewing", "authored"])
```

### PullRequestLabel

Source: `packages/contracts/src/pullRequest.ts:89`.

```text
Object({
  name: TrimmedNonEmptyString,
  color: Nullable(String),
})
```

### PullRequestLabelCandidate

Source: `packages/contracts/src/pullRequest.ts:208`.

```text
Object({
  ...PullRequestLabel.fields,
  description: Nullable(String),
  isApplied: Boolean,
})
```

### PullRequestLabelCandidateList

Source: `packages/contracts/src/pullRequest.ts:215`.

```text
Object({
  candidates: Array(PullRequestLabelCandidate),
  truncated: Boolean,
})
```

### PullRequestLabelChangeInput

Source: `packages/contracts/src/pullRequest.ts:845`.

```text
Object({
  ...PullRequestRef.fields,
  labels: Array(TrimmedNonEmptyString).check(isMinLength(1), isMaxLength(25)),
  applied: Boolean,
})
```

### PullRequestLineCount

Source: `packages/contracts/src/pullRequest.ts:682`.

```text
Object({
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
})
```

### PullRequestLinkedThreadsResult

Source: `packages/contracts/src/pullRequest.ts:427`.

```text
Object({
  threads: Array(
    Object({
      id: ThreadId,
      projectId: ProjectId,
      title: String,
      archivedAt: Nullable(IsoDateTime),
    }),
})
```

### PullRequestListCursors

Source: `packages/contracts/src/pullRequest.ts:345`.

```text
Record(
  TrimmedNonEmptyString,
  TrimmedNonEmptyString.check(isMaxLength(4096)),
)
```

### PullRequestListEntry

Source: `packages/contracts/src/pullRequest.ts:316`.

```text
Object({
  stack: Optional(PullRequestStackMembership),
  provider: SourceControlProviderKind,
  host: TrimmedNonEmptyString,
  projectId: ProjectId,
  projectTitle: TrimmedNonEmptyString,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  title: TrimmedNonEmptyString,
  url: TrimmedNonEmptyString,
  author: Nullable(PullRequestActor),
  headBranch: TrimmedNonEmptyString,
  baseBranch: TrimmedNonEmptyString,
  state: PullRequestState,
  isDraft: Boolean,
  mergeability: PullRequestMergeability,
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  observedAt: Optional(Finite),
  viewerReviewRequested: Boolean,
  labels: Array(PullRequestLabel),
  reviewDecision: Optional(PullRequestReviewDecision),
  checksState: Optional(PullRequestChecksState),
})
```

### PullRequestListFilters

Source: `packages/contracts/src/pullRequest.ts:38`.

```text
Object({
  draft: Optional(Enum(["only", "hide"])),
  review: Optional(
    Enum(["approved", "changes-requested", "review-required", "none"]),
  ),
  checks: Optional(Enum(["passing", "failing"])),
  labels: Optional(Array(PullRequestQualifierValues).check(isMaxLength(10))),
  excludedLabels: Optional(PullRequestQualifierValues),
  author: Optional(PullRequestQualifierValue),
})
```

### PullRequestListInput

Source: `packages/contracts/src/pullRequest.ts:351`.

```text
Object({
  state: PullRequestListState,
  involvement: Optional(PullRequestInvolvement),
  filters: Optional(PullRequestListFilters),
  projectId: Optional(ProjectId),
  projectIds: Optional(Array(ProjectId).check(isMaxLength(100))),
  host: Optional(TrimmedNonEmptyString),
  limit: Optional(Int.check(isBetween({ minimum: 1, maximum: 500 })),
  cursors: Optional(PullRequestListCursors),
  query: Optional(TrimmedNonEmptyString.check(isMaxLength(200))),
})
```

### PullRequestListProjectError

Source: `packages/contracts/src/pullRequest.ts:375`.

```text
Object({
  projectId: ProjectId,
  projectTitle: TrimmedNonEmptyString,
  message: TrimmedNonEmptyString,
})
```

### PullRequestListResult

Source: `packages/contracts/src/pullRequest.ts:382`.

```text
Object({
  viewers: Record(TrimmedNonEmptyString, TrimmedNonEmptyString),
  providers: Array(PullRequestProviderSummary),
  entries: Array(PullRequestListEntry),
  errors: Array(PullRequestListProjectError),
  truncated: Boolean,
  nextCursors: PullRequestListCursors,
})
```

### PullRequestListState

Source: `packages/contracts/src/pullRequest.ts:23`.

```text
Enum(["all", "open", "closed", "merged"])
```

### PullRequestListStatsInput

Source: `packages/contracts/src/pullRequest.ts:506`.

```text
Object({
  refs: Array(PullRequestRef).check(isMaxLength(500)),
})
```

### PullRequestListStatsResult

Source: `packages/contracts/src/pullRequest.ts:511`.

```text
Object({
  stats: Array(PullRequestDiffStat),
})
```

### PullRequestMergeCapabilities

Source: `packages/contracts/src/pullRequest.ts:301`.

```text
Object({
  merge: Boolean,
  squash: Boolean,
  rebase: Boolean,
})
```

### PullRequestMergeMethod

Source: `packages/contracts/src/pullRequest.ts:57`.

```text
Enum(["merge", "squash", "rebase"])
```

### PullRequestMergeability

Source: `packages/contracts/src/pullRequest.ts:54`.

```text
Enum(["mergeable", "conflicting", "unknown"])
```

### PullRequestOmittedFileStat

Source: `packages/contracts/src/pullRequest.ts:631`.

```text
Object({
  path: TrimmedNonEmptyString,
  additions: Number,
  deletions: Number,
})
```

### PullRequestPreview

Source: `packages/contracts/src/pullRequest.ts:439`.

```text
Object({
  projectId: ProjectId,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  title: TrimmedNonEmptyString,
  url: TrimmedNonEmptyString,
  author: Nullable(PullRequestActor),
  state: PullRequestState,
  isDraft: Boolean,
  createdAt: IsoDateTime,
})
```

### PullRequestProviderSummary

Source: `packages/contracts/src/pullRequest.ts:364`.

```text
Object({
  host: TrimmedNonEmptyString,
  kind: SourceControlProviderKind,
  searchesOnHost: Boolean,
  actions: Optional(ForwardCompatibleArray(PullRequestAction)),
  projectCount: PositiveInt,
  configured: Boolean,
  detail: Nullable(TrimmedNonEmptyString),
})
```

### PullRequestQualifierValue

Source: `packages/contracts/src/pullRequest.ts:33`.

```text
TrimmedNonEmptyString.check(isMaxLength(200))
```

### PullRequestQualifierValues

Source: `packages/contracts/src/pullRequest.ts:34`.

```text
Array(PullRequestQualifierValue).check(
  isMaxLength(10),
)
```

### PullRequestReaction

Source: `packages/contracts/src/pullRequest.ts:127`.

```text
Object({
  content: PullRequestReactionContent,
  count: PositiveInt,
  actors: Array(TrimmedNonEmptyString),
  viewerHasReacted: Boolean,
})
```

### PullRequestReactionContent

Source: `packages/contracts/src/pullRequest.ts:115`.

```text
Enum([
  "thumbs-up",
  "thumbs-down",
  "laugh",
  "hooray",
  "confused",
  "heart",
  "rocket",
  "eyes",
])
```

### PullRequestReactionInput

Source: `packages/contracts/src/pullRequest.ts:828`.

```text
Object({
  ...PullRequestRef.fields,
  subjectId: Optional(TrimmedNonEmptyString),
  content: PullRequestReactionContent,
  reacted: Boolean,
})
```

### PullRequestRef

Source: `packages/contracts/src/pullRequest.ts:394`.

```text
Object({
  projectId: ProjectId,
  host: Optional(TrimmedNonEmptyString),
  expectedAccountId: Optional(TrimmedNonEmptyString),
  allowStale: Optional(Boolean),
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
})
```

### PullRequestReportStateInput

Source: `packages/contracts/src/pullRequest.ts:546`.

```text
Object({
  reference: PullRequestRef,
  state: PullRequestState,
  isDraft: Optional(Boolean),
})
```

### PullRequestReviewCapabilities

Source: `packages/contracts/src/pullRequest.ts:231`.

```text
Object({
  inlineComment: Boolean,
  rangeComment: Optional(Boolean),
  reply: Boolean,
  resolve: Boolean,
  verdicts: ForwardCompatibleArray(PullRequestReviewVerdict),
})
```

### PullRequestReviewCommentDraft

Source: `packages/contracts/src/pullRequest.ts:779`.

```text
Object({
  path: TrimmedNonEmptyString,
  oldPath: Optional(TrimmedNonEmptyString),
  position: PullRequestReviewPosition,
  startPosition: Optional(PullRequestReviewPosition),
  body: CommentBody,
})
```

### PullRequestReviewDecision

Source: `packages/contracts/src/pullRequest.ts:26`.

```text
Enum([
  "approved",
  "changes-requested",
  "review-required",
])
```

### PullRequestReviewGuide

Source: `packages/contracts/src/pullRequestReviewGuide.ts:45`.

```text
Object({
  headSha: TrimmedNonEmptyString,
  generatedAt: IsoDateTime,
  modelSelection: ModelSelection,
  stops: Array(PullRequestReviewGuideStop),
})
```

### PullRequestReviewGuideAttention

Source: `packages/contracts/src/pullRequestReviewGuide.ts:6`.

```text
Object({
  kind: Enum(["tradeoff", "contract", "product", "risk", "accessibility"]),
  note: TrimmedNonEmptyString,
})
```

### PullRequestReviewGuideExcerpt

Source: `packages/contracts/src/pullRequestReviewGuide.ts:12`.

```text
Object({
  path: TrimmedNonEmptyString,
  side: Enum(["new", "old"]),
  start: PositiveInt,
  end: PositiveInt,
  caption: String,
})
```

### PullRequestReviewGuideNote

Source: `packages/contracts/src/pullRequestReviewGuide.ts:21`.

```text
Object({
  path: TrimmedNonEmptyString,
  line: PositiveInt,
  title: TrimmedNonEmptyString,
  body: String,
})
```

### PullRequestReviewGuideResult

Source: `packages/contracts/src/pullRequestReviewGuide.ts:66`.

```text
Object({
  guide: Nullable(PullRequestReviewGuide),
  run: Nullable(PullRequestReviewGuideRun),
  currentHeadSha: Nullable(TrimmedNonEmptyString),
})
```

### PullRequestReviewGuideRun

Source: `packages/contracts/src/pullRequestReviewGuide.ts:53`.

```text
Union([
  TaggedObject("Running", {
    headSha: TrimmedNonEmptyString,
    startedAt: IsoDateTime,
  }),
  TaggedObject("Failed", {
    headSha: TrimmedNonEmptyString,
    failedAt: IsoDateTime,
    detail: TrimmedNonEmptyString,
  }),
])
```

### PullRequestReviewGuideStop

Source: `packages/contracts/src/pullRequestReviewGuide.ts:29`.

```text
Object({
  title: TrimmedNonEmptyString,
  summary: String,
  attention: Nullable(PullRequestReviewGuideAttention),
  excerpts: Array(PullRequestReviewGuideExcerpt),
  files: Array(
    Object({
      path: TrimmedNonEmptyString,
      note: Optional(String),
    }),
  notes: Array(PullRequestReviewGuideNote),
  questions: Array(TrimmedNonEmptyString),
})
```

### PullRequestReviewPosition

Source: `packages/contracts/src/pullRequest.ts:761`.

```text
Union([
  Object({
    kind: Const("added"),
    newLine: PositiveInt,
  }),
  Object({
    kind: Const("deleted"),
    oldLine: PositiveInt,
  }),
  Object({
    kind: Const("context"),
    oldLine: PositiveInt,
    newLine: PositiveInt,
    side: PullRequestDiffSide,
  }),
])
```

### PullRequestReviewThread

Source: `packages/contracts/src/pullRequest.ts:176`.

```text
Object({
  id: TrimmedNonEmptyString,
  path: TrimmedNonEmptyString,
  line: Nullable(PositiveInt),
  side: PullRequestDiffSide,
  isResolved: Boolean,
  isOutdated: Boolean,
  comments: Array(PullRequestThreadComment),
  commentCount: Optional(NonNegativeInt),
  nextCommentsCursor: Optional(TrimmedNonEmptyString),
  canResolve: Optional(Boolean),
})
```

### PullRequestReviewVerdict

Source: `packages/contracts/src/pullRequest.ts:161`.

```text
Enum(["comment", "approve", "request-changes"])
```

### PullRequestReviewerCandidate

Source: `packages/contracts/src/pullRequest.ts:194`.

```text
Object({
  ...PullRequestActor.fields,
  id: TrimmedNonEmptyString,
  kind: PullRequestReviewerKind,
  isRequested: Boolean,
})
```

### PullRequestReviewerCandidateList

Source: `packages/contracts/src/pullRequest.ts:202`.

```text
Object({
  candidates: Array(PullRequestReviewerCandidate),
  truncated: Boolean,
})
```

### PullRequestReviewerCapabilities

Source: `packages/contracts/src/pullRequest.ts:250`.

```text
Object({
  request: Boolean,
  listCandidates: Boolean,
})
```

### PullRequestReviewerKind

Source: `packages/contracts/src/pullRequest.ts:191`.

```text
Enum(["user", "team"])
```

### PullRequestReviewerRequestInput

Source: `packages/contracts/src/pullRequest.ts:836`.

```text
Object({
  ...PullRequestRef.fields,
  reviewers: Array(
    Object({ id: TrimmedNonEmptyString, kind: PullRequestReviewerKind }).check(isMinLength(1), isMaxLength(25)),
  requested: Boolean,
})
```

### PullRequestRoutingIdentityInput

Source: `packages/contracts/src/pullRequest.ts:404`.

```text
Object({
  host: TrimmedNonEmptyString,
})
```

### PullRequestRoutingIdentityResult

Source: `packages/contracts/src/pullRequest.ts:409`.

```text
Object({
  accountId: TrimmedNonEmptyString,
  host: TrimmedNonEmptyString,
  provider: SourceControlProviderKind,
  viewer: TrimmedNonEmptyString,
})
```

### PullRequestRoutingResult

Source: `packages/contracts/src/pullRequest.ts:417`.

```text
Object({
  accountId: TrimmedNonEmptyString,
  host: TrimmedNonEmptyString,
  provider: SourceControlProviderKind,
  viewer: TrimmedNonEmptyString,
  projectTitle: TrimmedNonEmptyString,
  workspaceRoot: TrimmedNonEmptyString,
})
```

### PullRequestSetFilesViewedInput

Source: `packages/contracts/src/pullRequest.ts:708`.

```text
Object({
  ...PullRequestRef.fields,
  files: Array(
    Object({
      path: FilePath,
      viewed: Boolean,
    }).check(isMaxLength(MAX_FILES_VIEWED_PRESSES)),
})
```

### PullRequestStack

Source: `packages/contracts/src/pullRequest.ts:479`.

```text
Object({
  id: TrimmedNonEmptyString,
  number: PositiveInt,
  url: TrimmedNonEmptyString,
  base: TrimmedNonEmptyString,
  layers: Array(
    Object({
      number: PositiveInt,
      title: Optional(String),
      isDraft: Optional(Boolean),
      headSha: Optional(TrimmedNonEmptyString),
      headBranch: TrimmedNonEmptyString,
      state: PullRequestState,
    }),
})
```

### PullRequestStackHead

Source: `packages/contracts/src/pullRequest.ts:719`.

```text
Object({
  number: PositiveInt,
  headSha: TrimmedNonEmptyString,
})
```

### PullRequestStackMembership

Source: `packages/contracts/src/pullRequest.ts:308`.

```text
Object({
  number: PositiveInt,
  position: PositiveInt,
  size: PositiveInt,
  base: TrimmedNonEmptyString,
})
```

### PullRequestState

Source: `packages/contracts/src/pullRequest.ts:20`.

```text
Enum(["open", "closed", "merged"])
```

### PullRequestSubmitReviewInput

Source: `packages/contracts/src/pullRequest.ts:793`.

```text
Object({
  ...PullRequestRef.fields,
  verdict: PullRequestReviewVerdict,
  body: String.check(isMaxLength(65_536)),
  comments: Array(PullRequestReviewCommentDraft),
})
```

### PullRequestSummary

Source: `packages/contracts/src/pullRequest.ts:452`.

```text
Object({
  provider: SourceControlProviderKind,
  projectId: ProjectId,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  title: TrimmedNonEmptyString,
  url: TrimmedNonEmptyString,
  state: PullRequestState,
  isDraft: Optional(Boolean),
  headBranch: TrimmedNonEmptyString,
  baseBranch: TrimmedNonEmptyString,
  closedAt: Optional(Nullable(String)),
  mergedAt: Optional(Nullable(String)),
  updatedAt: IsoDateTime,
  observedAt: Optional(Finite),
  author: Optional(Nullable(PullRequestActor)),
  additions: Optional(NonNegativeInt),
  deletions: Optional(NonNegativeInt),
  changedFiles: Optional(NonNegativeInt),
  reviewDecision: Optional(Nullable(PullRequestReviewDecision)),
  checksState: Optional(Nullable(PullRequestChecksState)),
  mergeability: Optional(PullRequestMergeability),
  stack: Optional(Nullable(PullRequestStackMembership)),
})
```

### PullRequestThreadComment

Source: `packages/contracts/src/pullRequest.ts:164`.

```text
Object({
  id: TrimmedNonEmptyString,
  author: Nullable(PullRequestActor),
  body: String,
  createdAt: IsoDateTime,
  editedAt: Optional(Nullable(IsoDateTime)),
  url: Nullable(String),
  reactions: Optional(Array(PullRequestReaction)),
  canEdit: Optional(Boolean),
})
```

### PullRequestThreadCommentsInput

Source: `packages/contracts/src/pullRequest.ts:801`.

```text
Object({
  ...PullRequestRef.fields,
  threadId: TrimmedNonEmptyString,
  cursor: TrimmedNonEmptyString,
})
```

### PullRequestThreadCommentsResult

Source: `packages/contracts/src/pullRequest.ts:808`.

```text
Object({
  comments: Array(PullRequestThreadComment),
  nextCursor: Nullable(TrimmedNonEmptyString),
})
```

### PullRequestThreadReplyInput

Source: `packages/contracts/src/pullRequest.ts:814`.

```text
Object({
  ...PullRequestRef.fields,
  threadId: TrimmedNonEmptyString,
  body: CommentBody,
})
```

### PullRequestThreadResolutionInput

Source: `packages/contracts/src/pullRequest.ts:821`.

```text
Object({
  ...PullRequestRef.fields,
  threadId: TrimmedNonEmptyString,
  resolved: Boolean,
})
```

### PullRequestUpdateInput

Source: `packages/contracts/src/pullRequest.ts:745`.

```text
Object({
  ...PullRequestRef.fields,
  title: Optional(TrimmedNonEmptyString.check(isMaxLength(1024))),
  body: Optional(String.check(isMaxLength(65_536))),
})
```

### PullRequestUpdateMethod

Source: `packages/contracts/src/pullRequest.ts:75`.

```text
Enum(["merge", "rebase"])
```

### PullRequestViewedFilesStore

Source: `packages/contracts/src/pullRequest.ts:256`.

```text
Enum(["host", "environment"])
```

### PullRequestViewerPermissions

Source: `packages/contracts/src/pullRequest.ts:288`.

```text
Object({
  stackRebase: Optional(Boolean),
  actions: ForwardCompatibleArray(PullRequestAction),
  comment: Boolean,
  resolve: Boolean,
  verdicts: ForwardCompatibleArray(PullRequestReviewVerdict),
  requestReviewers: Boolean,
  updateMethods: Optional(ForwardCompatibleArray(PullRequestUpdateMethod)),
  labels: Optional(Boolean),
  editChangeRequest: Optional(Boolean),
})
```

### ReceivedProjectIcon

Source: `packages/contracts/src/project.ts:138`.

```text
ForwardCompatibleUnion(projectIconMembers, "kind")
```

### RelayClientInstallFailedError

Source: `packages/contracts/src/relayClient.ts:57`.

```text
Object({_tag: Const("RelayClientInstallFailedError"),
    reason: RelayClientInstallFailureReasonSchema,
    message: String,
  })
```

### RelayClientInstallFailureReasonSchema

Source: `packages/contracts/src/relayClient.ts:46`.

```text
Enum([
  "download_failed",
  "invalid_checksum",
  "install_locked",
  "override_missing",
  "unsupported_platform",
  "validation_failed",
  "write_failed",
])
```

### RelayClientInstallProgressEventSchema

Source: `packages/contracts/src/relayClient.ts:34`.

```text
Union([
  Object({
    type: Const("progress"),
    stage: RelayClientInstallProgressStageSchema,
  }),
  Object({
    type: Const("complete"),
    status: RelayClientStatusSchema,
  }),
])
```

### RelayClientInstallProgressStageSchema

Source: `packages/contracts/src/relayClient.ts:23`.

```text
Enum([
  "checking",
  "waiting_for_lock",
  "downloading",
  "verifying",
  "installing",
  "validating",
  "activating",
])
```

### RelayClientStatusSchema

Source: `packages/contracts/src/relayClient.ts:3`.

```text
Union([
  Object({
    status: Const("available"),
    executablePath: String,
    source: Enum(["override", "managed", "path"]),
    version: String,
  }),
  Object({
    status: Const("missing"),
    version: String,
  }),
  Object({
    status: Const("unsupported"),
    platform: String,
    arch: String,
    version: String,
  }),
])
```

### RelayCloudEnvironmentHealthRequest

Source: `packages/contracts/src/relay.ts:901`.

```text
Object({
  proof: RelayCloudEnvironmentHealthProof,
})
```

### RelayCloudMintCredentialRequest

Source: `packages/contracts/src/relay.ts:874`.

```text
Object({
  proof: RelayCloudMintCredentialProof,
})
```

### RelayEnvironmentConfigRequest

Source: `packages/contracts/src/relay.ts:222`.

```text
Object({
  relayUrl: String,
  relayIssuer: Optional(String),
  cloudUserId: String,
  environmentCredential: String,
  cloudMintPublicKey: String,
  endpointRuntime: Nullable(RelayManagedEndpointRuntimeConfig),
})
```

### RelayEnvironmentHealthResponse

Source: `packages/contracts/src/relay.ts:917`.

```text
Object({
  environmentId: EnvironmentId,
  status: Const("online"),
  descriptor: ExecutionEnvironmentDescriptor,
  checkedAt: TrimmedNonEmptyString,
  proof: TrimmedNonEmptyString,
})
```

### RelayEnvironmentMintResponse

Source: `packages/contracts/src/relay.ts:936`.

```text
Object({
  credential: TrimmedNonEmptyString,
  expiresAt: TrimmedNonEmptyString,
  proof: TrimmedNonEmptyString,
})
```

### RelayLinkProofRequest

Source: `packages/contracts/src/relay.ts:214`.

```text
Object({
  challenge: String,
  relayIssuer: String,
  endpoint: RelayManagedEndpoint,
  origin: RelayManagedEndpointOrigin,
})
```

### RelayManagedEndpoint

Source: `packages/contracts/src/relay.ts:162`.

```text
Object({
  httpBaseUrl: TrimmedNonEmptyString,
  wsBaseUrl: TrimmedNonEmptyString,
  providerKind: RelayManagedEndpointProviderKind,
})
```

### RelayManagedEndpointOrigin

Source: `packages/contracts/src/relay.ts:169`.

```text
Object({
  localHttpHost: TrimmedNonEmptyString,
  localHttpPort: Int.check(
    isGreaterThanOrEqualTo(1),
    isLessThanOrEqualTo(65_535),
  ),
})
```

### RelayManagedEndpointProviderKind

Source: `packages/contracts/src/relay.ts:155`.

```text
Enum([
  "manual",
  "cloudflare_tunnel",
  "t3_relay",
])
```

### RelayManagedEndpointRuntimeConfig

Source: `packages/contracts/src/relay.ts:178`.

```text
Object({
  providerKind: RelayManagedEndpointProviderKind,
  connectorToken: TrimmedNonEmptyString,
  tunnelId: Optional(TrimmedNonEmptyString),
  tunnelName: Optional(TrimmedNonEmptyString),
})
```

### RemoteOpenTarget

Source: `packages/contracts/src/editor.ts:225`.

```text
Object({
  kind: RemoteOpenTargetKind,
  host: TrimmedNonEmptyString,
  username: Optional(TrimmedNonEmptyString),
})
```

### RemoteOpenTargetKind

Source: `packages/contracts/src/editor.ts:222`.

```text
Enum(["tailscale", "mdns"])
```

### RepositoryIdentity

Source: `packages/contracts/src/environment.ts:164`.

```text
Object({
  canonicalKey: TrimmedNonEmptyString,
  locator: RepositoryIdentityLocator,
  webUrl: Optional(TrimmedNonEmptyString),
  rootPath: Optional(TrimmedNonEmptyString),
  displayName: Optional(TrimmedNonEmptyString),
  provider: Optional(SourceControlProviderKind),
  owner: Optional(TrimmedNonEmptyString),
  name: Optional(TrimmedNonEmptyString),
  origin: Optional(RepositoryOrigin),
})
```

### RepositoryIdentityLocator

Source: `packages/contracts/src/environment.ts:150`.

```text
Object({
  source: Const("git-remote"),
  remoteName: TrimmedNonEmptyString,
  remoteUrl: TrimmedNonEmptyString,
})
```

### RepositoryOrigin

Source: `packages/contracts/src/environment.ts:157`.

```text
Object({
  canonicalKey: TrimmedNonEmptyString,
  remoteUrl: Optional(TrimmedNonEmptyString),
  displayName: Optional(TrimmedNonEmptyString),
})
```

### ResolvedKeybindingRule

Source: `packages/contracts/src/keybindings.ts:199`.

```text
Object({
  command: KeybindingCommand,
  shortcut: KeybindingShortcut,
  whenAst: Optional(KeybindingWhenNode),
})
```

### ResolvedKeybindingsConfig

Source: `packages/contracts/src/keybindings.ts:207`.

```text
ForwardCompatibleArray(ResolvedKeybindingRule).check(
  isMaxLength(MAX_KEYBINDINGS_COUNT),
)
```

### ResourceAttributionEntry

Source: `packages/contracts/src/resourceTelemetry.ts:423`.

```text
Object({
  component: TrimmedNonEmptyString,
  operation: TrimmedNonEmptyString,
  logicalReadBytes: NonNegativeInt,
  logicalWriteBytes: NonNegativeInt,
  count: NonNegativeInt,
  durationMs: NonNegativeInt,
})
```

### ResourceAttributionSnapshot

Source: `packages/contracts/src/resourceTelemetry.ts:433`.

```text
Object({
  readAt: DateTimeUtc,
  entries: Array(ResourceAttributionEntry),
})
```

### ResourceTelemetryAggregate

Source: `packages/contracts/src/resourceTelemetry.ts:380`.

```text
Object({
  processCount: NonNegativeInt,
  currentCpuPercent: Number,
  cpuTimeMs: NonNegativeInt,
  currentRssBytes: NonNegativeInt,
  peakRssBytes: NonNegativeInt,
  ioReadBytes: NonNegativeInt,
  ioWriteBytes: NonNegativeInt,
  ioReadBytesPerSecond: Number,
  ioWriteBytesPerSecond: Number,
  processStarts: NonNegativeInt,
  processExits: NonNegativeInt,
})
```

### ResourceTelemetryGroups

Source: `packages/contracts/src/resourceTelemetry.ts:395`.

```text
Object({
  backend: ResourceTelemetryAggregate,
  electron: ResourceTelemetryAggregate,
  monitor: ResourceTelemetryAggregate,
  allT3: ResourceTelemetryAggregate,
})
```

### ResourceTelemetryHealth

Source: `packages/contracts/src/resourceTelemetry.ts:410`.

```text
Object({
  native: ResourceTelemetrySourceHealth,
  desktop: ResourceTelemetrySourceHealth,
  sidecarVersion: Option(TrimmedNonEmptyString),
  sidecarPid: Option(PositiveInt),
  restartCount: NonNegativeInt,
  collectionDurationMicros: NonNegativeInt,
  scannedProcessCount: NonNegativeInt,
  retainedProcessCount: NonNegativeInt,
  inaccessibleProcessCount: NonNegativeInt,
})
```

### ResourceTelemetryHistory

Source: `packages/contracts/src/resourceTelemetry.ts:491`.

```text
Object({
  readAt: DateTimeUtc,
  windowMs: NonNegativeInt,
  bucketMs: NonNegativeInt,
  sampleIntervalMs: NonNegativeInt,
  retainedSampleCount: NonNegativeInt,
  buckets: Array(ResourceTelemetryHistoryBucket),
  topProcesses: Array(ResourceTelemetryProcessSummary),
  health: ResourceTelemetryHealth,
})
```

### ResourceTelemetryHistoryBucket

Source: `packages/contracts/src/resourceTelemetry.ts:457`.

```text
Object({
  startedAt: DateTimeUtc,
  endedAt: DateTimeUtc,
  avgCpuPercent: Number,
  maxCpuPercent: Number,
  maxRssBytes: NonNegativeInt,
  ioReadBytes: NonNegativeInt,
  ioWriteBytes: NonNegativeInt,
  maxProcessCount: NonNegativeInt,
})
```

### ResourceTelemetryHistoryInput

Source: `packages/contracts/src/resourceTelemetry.ts:451`.

```text
Object({
  windowMs: NonNegativeInt,
  bucketMs: NonNegativeInt,
})
```

### ResourceTelemetryIoSemantics

Source: `packages/contracts/src/resourceTelemetry.ts:18`.

```text
Enum([
  "storage",
  "logical",
  "all-io",
  "unavailable",
])
```

### ResourceTelemetryProcess

Source: `packages/contracts/src/resourceTelemetry.ts:352`.

```text
Object({
  identity: ResourceTelemetryProcessIdentity,
  ppid: NonNegativeInt,
  childPids: Array(PositiveInt),
  depth: NonNegativeInt,
  name: String,
  command: String,
  status: String,
  category: ResourceTelemetryProcessCategory,
  electronType: Optional(DesktopElectronProcessType),
  electronServiceName: Optional(String),
  cpuPercent: Number,
  cpuTimeMs: NonNegativeInt,
  residentBytes: NonNegativeInt,
  peakResidentBytes: NonNegativeInt,
  virtualBytes: NonNegativeInt,
  ioReadBytes: NonNegativeInt,
  ioWriteBytes: NonNegativeInt,
  ioReadBytesPerSecond: Number,
  ioWriteBytesPerSecond: Number,
  ioSemantics: ResourceTelemetryIoSemantics,
  idleWakeupsPerSecond: Optional(Number),
  runTimeMs: NonNegativeInt,
  firstSeenAt: DateTimeUtc,
  lastSeenAt: DateTimeUtc,
})
```

### ResourceTelemetryProcessCategory

Source: `packages/contracts/src/resourceTelemetry.ts:26`.

```text
Enum([
  "server",
  "server-child",
  "provider-root",
  "terminal-root",
  "electron-main",
  "electron-renderer",
  "electron-gpu",
  "electron-utility",
  "resource-monitor",
  "unknown-t3",
])
```

### ResourceTelemetryProcessIdentity

Source: `packages/contracts/src/resourceTelemetry.ts:49`.

```text
Object({
  pid: PositiveInt,
  startTimeMs: NonNegativeInt,
})
```

### ResourceTelemetryProcessSummary

Source: `packages/contracts/src/resourceTelemetry.ts:469`.

```text
Object({
  identity: ResourceTelemetryProcessIdentity,
  ppid: NonNegativeInt,
  depth: NonNegativeInt,
  name: String,
  command: String,
  category: ResourceTelemetryProcessCategory,
  firstSeenAt: DateTimeUtc,
  lastSeenAt: DateTimeUtc,
  currentCpuPercent: Number,
  avgCpuPercent: Number,
  maxCpuPercent: Number,
  cpuTimeMs: NonNegativeInt,
  currentRssBytes: NonNegativeInt,
  peakRssBytes: NonNegativeInt,
  ioReadBytes: NonNegativeInt,
  ioWriteBytes: NonNegativeInt,
  ioSemantics: ResourceTelemetryIoSemantics,
  sampleCount: NonNegativeInt,
})
```

### ResourceTelemetryRetryResult

Source: `packages/contracts/src/resourceTelemetry.ts:503`.

```text
Object({
  accepted: Boolean,
  snapshot: ResourceTelemetrySnapshot,
})
```

### ResourceTelemetrySnapshot

Source: `packages/contracts/src/resourceTelemetry.ts:439`.

```text
Object({
  readAt: DateTimeUtc,
  sampleIntervalMs: NonNegativeInt,
  processes: Array(ResourceTelemetryProcess),
  groups: ResourceTelemetryGroups,
  power: HostPowerSnapshot,
  speedLimitPercent: Option(Number),
  attribution: ResourceAttributionSnapshot,
  health: ResourceTelemetryHealth,
})
```

### ResourceTelemetrySourceHealth

Source: `packages/contracts/src/resourceTelemetry.ts:403`.

```text
Object({
  status: ResourceTelemetrySourceStatus,
  lastSampleAt: Option(DateTimeUtc),
  lastError: Option(TrimmedNonEmptyString),
})
```

### ResourceTelemetrySourceStatus

Source: `packages/contracts/src/resourceTelemetry.ts:40`.

```text
Enum([
  "starting",
  "healthy",
  "degraded",
  "unavailable",
  "stopped",
])
```

### ResponseStreamingMode

Source: `packages/contracts/src/settings.ts:916`.

```text
Enum(["turn", "paragraph"])
```

### ReviewCommentContextRecord

Source: `packages/contracts/src/composerContext.ts:175`.

```text
Object({
  ...recordBase,
  kind: Const("review-comment"),
  sectionId: TrimmedNonEmptyString.check(isMaxLength(255)),
  sectionTitle: ShortString,
  filePath: TrimmedNonEmptyString.check(isMaxLength(2_048)),
  startIndex: NonNegativeInt,
  endIndex: NonNegativeInt,
  rangeLabel: ShortString,
  text: BoundedString(COMPOSER_CONTEXT_REVIEW_TEXT_MAX_CHARS),
  diff: BoundedString(COMPOSER_CONTEXT_REVIEW_DIFF_MAX_CHARS),
  fenceLanguage: Optional(BoundedString(64)),
  pullRequest: Optional(PullRequestContextMetadata),
}).check(Predicate('semantic validation; see constraints in sections 2, 5 and 7'))
```

### ReviewDiffFileContentsInput

Source: `packages/contracts/src/review.ts:44`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  sourceKind: ReviewDiffPreviewSourceKind,
  changeType: Enum(["change", "rename-pure", "rename-changed", "new", "deleted"]),
  baseRef: Nullable(TrimmedNonEmptyString),
  headRef: Nullable(TrimmedNonEmptyString),
  oldPath: TrimmedNonEmptyString,
  newPath: TrimmedNonEmptyString,
})
```

### ReviewDiffFileContentsResult

Source: `packages/contracts/src/review.ts:55`.

```text
Object({
  oldContents: String,
  newContents: String,
})
```

### ReviewDiffFileStat

Source: `packages/contracts/src/review.ts:23`.

```text
Object({
  path: String,
  previousPath: Nullable(String),
  additions: Number,
  deletions: Number,
})
```

### ReviewDiffPreviewError

Source: `packages/contracts/src/review.ts:68`.

```text
Union([VcsError, GitCommandError])
```

### ReviewDiffPreviewInput

Source: `packages/contracts/src/review.ts:6`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  baseRef: Optional(TrimmedNonEmptyString),
  ignoreWhitespace: Optional(Boolean),
  file: Optional(
    Object({
      path: NonEmptyString,
      previousPath: Nullable(NonEmptyString),
      sourceKind: Enum(["working-tree", "branch-range"]),
    }),
})
```

### ReviewDiffPreviewResult

Source: `packages/contracts/src/review.ts:61`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  generatedAt: DateTimeUtc,
  sources: Array(ReviewDiffPreviewSource),
})
```

### ReviewDiffPreviewSource

Source: `packages/contracts/src/review.ts:31`.

```text
Object({
  id: TrimmedNonEmptyString,
  kind: ReviewDiffPreviewSourceKind,
  title: TrimmedNonEmptyString,
  baseRef: Nullable(TrimmedNonEmptyString),
  headRef: Nullable(TrimmedNonEmptyString),
  diff: String,
  diffHash: TrimmedNonEmptyString,
  truncated: Boolean,
  files: Optional(Array(ReviewDiffFileStat)),
})
```

### ReviewDiffPreviewSourceKind

Source: `packages/contracts/src/review.ts:20`.

```text
Enum(["working-tree", "branch-range"])
```

### RpcClientId

Source: `packages/contracts/src/baseSchemas.ts:276`.

```text
NonNegativeInt
```

### RuntimeMode

Source: `packages/contracts/src/providerPolicy.ts:25`.

```text
Enum([
  "approval-required",
  "auto-accept-edits",
  "auto",
  "full-access",
])
```

### SCRIPT_RUN_COMMAND_PATTERN

Source: `packages/contracts/src/keybindings.ts:118`.

```text
TemplateConst([
  Const("script."),
  NonEmptyString.check(
    isMaxLength(MAX_SCRIPT_ID_LENGTH),
    isPattern(/^[a-z0-9][a-z0-9-]*$/),
  ),
  Const(".run"),
])
```

### SECRET_REQUEST_FAILURE_MESSAGES

Source: `packages/contracts/src/secretRequest.ts:16`.

```text
{
  load_failed: "Could not load the secret request.",
  not_found: "This secret request no longer exists.",
  already_answered: "This secret request was already answered.",
  agent_stopped: "The agent that asked has stopped, so this secret can't be used.",
  store_failed: "Could not store the secret.",
  record_failed: "Saved the secret, but could not update the request.",
  invalid_ref: "That secretRef is not valid.",
  read_failed: "Could not read the secret.",
  ref_unavailable:
    "That secretRef was already used or does not exist. Ask the user again with request_secret.",
  ref_expired: "That secretRef expired. Ask the user again with request_secret.",
  consume_failed: "Could not use that secretRef. Try again.",
  not_a_file_request:
    'The user did not save this secret for shell use. Ask again with request_secret and delivery: "file".',
}
```

### SNAP_SHOT_ACCESSIBILITY_MAX_NODES

Source: `packages/contracts/src/chatAttachment.ts:41`.

```text
10_000
```

### SNAP_SHOT_ACCESSIBILITY_MAX_SERIALIZED_CHARS

Source: `packages/contracts/src/chatAttachment.ts:42`.

```text
32_000
```

### SNAP_SHOT_ACCESSIBLE_TEXT_MAX_CHARS

Source: `packages/contracts/src/chatAttachment.ts:40`.

```text
32_000
```

### SOURCE_CONTROL_MEDIA_ASSET

Source: `packages/contracts/src/assets.ts:18`.

```text
"github-media"
```

### STATIC_KEYBINDING_COMMANDS

Source: `packages/contracts/src/keybindings.ts:59`.

```text
[
  "sidebar.toggle",
  "navigation.back",
  "navigation.forward",
  "terminal.toggle",
  "terminal.split",
  "terminal.splitVertical",
  "terminal.new",
  "terminal.close",
  "rightPanel.toggle",
  "rightPanel.new",
  "threadPanel.toggle",
  "rightPanel.toggleMaximized",
  "rightPanel.close",
  "view.reopenClosed",
  "pullRequest.copyNumber",
  "diff.toggle",
  "preview.toggle",
  "preview.refresh",
  "preview.focusUrl",
  "preview.zoomIn",
  "preview.zoomOut",
  "preview.resetZoom",
  "commandPalette.toggle",
  "filePicker.toggle",
  "projectSearch.toggle",
  "usage.open",
  "theme.select",
  "appearance.cycle",
  "themeEditor.toggle",
  "composer.stash",
  "composer.sendAlternate",
  "composer.sendBackground",
  "composer.sendAndNewThread",
  "composer.host",
  "composer.cycleHost",
  "composer.effort",
  "composer.increaseEffort",
  "composer.decreaseEffort",
  "composer.mode",
  "composer.workspace",
  "composer.previousWorktree",
  "composer.branch",
  "chat.new",
  "chat.newLocal",
  "chat.newWithoutProject",
  "chat.find",
  "editor.openFavorite",
  "usage.cost",
  "usage.tokens",
  "usage.limits",
  "usage.period.day",
  "usage.period.week",
  "usage.period.month",
  "usage.period.quarter",
  ...MODEL_PICKER_KEYBINDING_COMMANDS,
  ...THREAD_KEYBINDING_COMMANDS,
]
```

### ScheduledTask

Source: `packages/contracts/src/scheduledTask.ts:197`.

```text
Object({
  id: ScheduledTaskId,
  title: TrimmedNonEmptyString,
  prompt: TrimmedNonEmptyString,
  enabled: Boolean,
  schedule: ScheduledTaskSchedule,
  projectId: ProjectId,
  threadId: Nullable(ThreadId),
  workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  createdBy: OrchestrationV2Actor,
  creationSource: OrchestrationV2CreationSource,
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  nextRunAt: Nullable(IsoDateTime),
  lastRunAt: Nullable(IsoDateTime),
  lastRunStatus: ScheduledTaskRunStatus,
  lastRunError: Nullable(String),
  runCount: Int.check(isGreaterThanOrEqualTo(0)),
  webhook: Optional(ScheduledTaskWebhookEndpoint),
})
```

### ScheduledTaskDeleteInput

Source: `packages/contracts/src/scheduledTask.ts:257`.

```text
Object({
  id: ScheduledTaskId,
})
```

### ScheduledTaskDeleteResult

Source: `packages/contracts/src/scheduledTask.ts:352`.

```text
Object({
  id: ScheduledTaskId,
})
```

### ScheduledTaskError

Source: `packages/contracts/src/scheduledTask.ts:362`.

```text
Object({_tag: Const("ScheduledTaskError"),
    message: String,
    taskId: Optional(ScheduledTaskId),
    cause: Optional(Defect()),
  })
```

### ScheduledTaskFixedTimeSchedule

Source: `packages/contracts/src/scheduledTask.ts:34`.

```text
Object({
  type: Const("fixed_time"),
  timeOfDay: TimeOfDay,
  weekdays: Optional(
    Array(
      Int.check(isBetween({ minimum: 0, maximum: 6 }),
    ),
  ),
})
```

### ScheduledTaskGetWebhookDeliveryInput

Source: `packages/contracts/src/scheduledTask.ts:334`.

```text
Object({
  id: ScheduledTaskId,
  deliveryId: ScheduledTaskWebhookDeliveryId,
  acceptAnyOutcome: Optional(Boolean),
})
```

### ScheduledTaskGetWebhookDeliveryResult

Source: `packages/contracts/src/scheduledTask.ts:341`.

```text
Object({
  delivery: ScheduledTaskWebhookDelivery,
})
```

### ScheduledTaskIntervalMs

Source: `packages/contracts/src/scheduledTask.ts:27`.

```text
Int.check(isGreaterThan(0))
```

### ScheduledTaskIntervalSchedule

Source: `packages/contracts/src/scheduledTask.ts:29`.

```text
Object({
  type: Const("interval"),
  everyMs: ScheduledTaskIntervalMs,
})
```

### ScheduledTaskListInput

Source: `packages/contracts/src/scheduledTask.ts:222`.

```text
Object({})
```

### ScheduledTaskListResult

Source: `packages/contracts/src/scheduledTask.ts:225`.

```text
Object({
  tasks: ForwardCompatibleArray(ScheduledTask),
})
```

### ScheduledTaskListWebhookDeliveriesInput

Source: `packages/contracts/src/scheduledTask.ts:316`.

```text
Object({
  id: ScheduledTaskId,
  acceptAnyOutcome: Optional(Boolean),
})
```

### ScheduledTaskListWebhookDeliveriesResult

Source: `packages/contracts/src/scheduledTask.ts:328`.

```text
Object({
  deliveries: Array(ScheduledTaskWebhookDeliverySummary),
})
```

### ScheduledTaskMutationResult

Source: `packages/contracts/src/scheduledTask.ts:347`.

```text
Object({
  task: ScheduledTask,
})
```

### ScheduledTaskRotateWebhookTokenInput

Source: `packages/contracts/src/scheduledTask.ts:267`.

```text
Object({
  id: ScheduledTaskId,
})
```

### ScheduledTaskRunNowInput

Source: `packages/contracts/src/scheduledTask.ts:262`.

```text
Object({
  id: ScheduledTaskId,
})
```

### ScheduledTaskRunNowResult

Source: `packages/contracts/src/scheduledTask.ts:357`.

```text
Object({
  task: ScheduledTask,
})
```

### ScheduledTaskRunStatus

Source: `packages/contracts/src/scheduledTask.ts:187`.

```text
Enum(["never", "running", "succeeded", "failed"])
```

### ScheduledTaskSchedule

Source: `packages/contracts/src/scheduledTask.ts:166`.

```text
Union([
  ScheduledTaskIntervalSchedule,
  ScheduledTaskFixedTimeSchedule,
  ScheduledTaskWebhookSchedule,
])
```

### ScheduledTaskSetEnabledInput

Source: `packages/contracts/src/scheduledTask.ts:251`.

```text
Object({
  id: ScheduledTaskId,
  enabled: Boolean,
})
```

### ScheduledTaskUpsertInput

Source: `packages/contracts/src/scheduledTask.ts:230`.

```text
Object({
  id: Optional(ScheduledTaskId),
  requireExisting: Optional(Boolean),
  commandId: Optional(CommandId),
  title: TrimmedNonEmptyString,
  prompt: TrimmedNonEmptyString,
  enabled: Boolean,
  schedule: ScheduledTaskUpsertSchedule,
  projectId: ProjectId,
  threadId: Optional(Nullable(ThreadId)),
  workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  createdBy: Optional(OrchestrationV2Actor),
  creationSource: Optional(OrchestrationV2CreationSource),
})
```

### ScheduledTaskUpsertSchedule

Source: `packages/contracts/src/scheduledTask.ts:173`.

```text
Union([
  Object({
    type: Const("interval"),
    everyMs: ScheduledTaskIntervalMs.check(
      isGreaterThanOrEqualTo(MIN_SCHEDULED_TASK_INTERVAL_MS),
    ),
  }),
  ScheduledTaskFixedTimeSchedule,
  ScheduledTaskUpsertWebhookSchedule,
])
```

### ScheduledTaskUpsertWebhookSchedule

Source: `packages/contracts/src/scheduledTask.ts:140`.

```text
Object({
  type: Const("webhook"),
  signature: Optional(
    Nullable(
      Object({
        ...ScheduledTaskWebhookSignatureFields,
        secret: Optional(TrimmedNonEmptyString),
        secretRef: Optional(SecretRef),
      }),
  ),
  maxDeliveryAgeMinutes: Optional(Nullable(WebhookMaxDeliveryAgeMinutes)),
  dedupe: Optional(ScheduledTaskWebhookDedupe),
  filters: Optional(ScheduledTaskWebhookFilters),
})
```

### ScheduledTaskWebhookDedupe

Source: `packages/contracts/src/scheduledTask.ts:81`.

```text
Object({
  replayKey: Optional(WebhookDedupeTemplate),
  coalesceKey: Optional(WebhookDedupeTemplate),
  coalesceWindowSeconds: Optional(
    Int.check(isBetween({ minimum: 10, maximum: 3600 }),
  ),
})
```

### ScheduledTaskWebhookDelivery

Source: `packages/contracts/src/scheduledTask.ts:306`.

```text
Object({
  ...ScheduledTaskWebhookDeliverySummary.fields,
  query: String,
  headers: Record(String, String),
  body: String,
  bodyTruncated: Boolean,
  renderedPrompt: Nullable(String),
})
```

### ScheduledTaskWebhookDeliveryId

Source: `packages/contracts/src/scheduledTask.ts:272`.

```text
TrimmedNonEmptyString
```

### ScheduledTaskWebhookDeliveryOutcome

Source: `packages/contracts/src/scheduledTask.ts:277`.

```text
Enum([
  "accepted",
  "dispatch_failed",
  "duplicate",
  "coalesced",
  "rejected_signature",
  "disabled",
  "rate_limited",
  "expired",
  "filtered",
])
```

### ScheduledTaskWebhookDeliverySummary

Source: `packages/contracts/src/scheduledTask.ts:290`.

```text
Object({
  id: ScheduledTaskWebhookDeliveryId,
  taskId: ScheduledTaskId,
  receivedAt: IsoDateTime,
  method: TrimmedNonEmptyString,
  contentType: Nullable(String),
  bodyBytes: Int.check(isGreaterThanOrEqualTo(0)),
  outcome: Union([ScheduledTaskWebhookDeliveryOutcome, String]),
  signatureVerified: Boolean,
  pendingWindow: Optional(Boolean),
  missingFields: Array(String),
  error: Nullable(String),
})
```

### ScheduledTaskWebhookEndpoint

Source: `packages/contracts/src/scheduledTask.ts:190`.

```text
Object({
  path: TrimmedNonEmptyString,
  url: Nullable(TrimmedNonEmptyString),
  hasSecret: Boolean,
})
```

### ScheduledTaskWebhookFilter

Source: `packages/contracts/src/scheduledTask.ts:105`.

```text
Union([
  Object({
    path: WebhookFilterPath,
    op: Enum(["equals", "notEquals", "startsWith"]),
    value: String,
  }),
  Object({
    path: WebhookFilterPath,
    op: Const("in"),
    value: Array(WebhookFilterListValue).check(isMinLength(1)),
  }),
  Object({ path: WebhookFilterPath, op: Const("exists") }),
])
```

### ScheduledTaskWebhookFilters

Source: `packages/contracts/src/scheduledTask.ts:124`.

```text
Array(ScheduledTaskWebhookFilter)
  .check(isMaxLength(MAX_WEBHOOK_FILTERS))
  
```

### ScheduledTaskWebhookSchedule

Source: `packages/contracts/src/scheduledTask.ts:130`.

```text
Object({
  type: Const("webhook"),
  signature: Nullable(ScheduledTaskWebhookSignature),
  maxDeliveryAgeMinutes: Optional(Nullable(WebhookMaxDeliveryAgeMinutes)),
  dedupe: Optional(ScheduledTaskWebhookDedupe),
  filters: Optional(ScheduledTaskWebhookFilters),
})
```

### ScheduledTaskWebhookSignature

Source: `packages/contracts/src/scheduledTask.ts:58`.

```text
Object(
  ScheduledTaskWebhookSignatureFields,
)
```

### ScheduledTaskWebhookSignatureFields

Source: `packages/contracts/src/scheduledTask.ts:48`.

```text
{
  header: TrimmedNonEmptyString,
  encoding: Enum(["hex", "base64"]),
  prefix: String,
}
```

### SecretRequestAnswerInput

Source: `packages/contracts/src/secretRequest.ts:6`.

```text
Object({
  threadId: ThreadId,
  turnItemId: TurnItemId,
  answer: Union([
    Object({ type: Const("save"), secret: TrimmedNonEmptyString }),
    Object({ type: Const("decline") }),
  ]),
})
```

### SecretRequestError

Source: `packages/contracts/src/secretRequest.ts:40`.

```text
Object({_tag: Const("SecretRequestError"),
    reason: SecretRequestFailureReason,
    cause: Optional(Defect()),
  })
```

### SecretRequestFailureReason

Source: `packages/contracts/src/secretRequest.ts:33`.

```text
Enum(["load_failed","not_found","already_answered","agent_stopped","store_failed","record_failed","invalid_ref","read_failed","ref_unavailable","ref_expired","consume_failed","not_a_file_request"])
```

### SelectProviderOptionDescriptor

Source: `packages/contracts/src/model.ts:24`.

```text
Object({
  ...ProviderOptionDescriptorBase,
  type: Const("select"),
  options: Array(ProviderOptionChoice),
  currentValue: Optional(TrimmedNonEmptyString),
  promptInjectedValues: Optional(Array(TrimmedNonEmptyString)),
})
```

### ServerAuthBootstrapMethod

Source: `packages/contracts/src/auth.ts:20`.

```text
Enum(["desktop-bootstrap", "one-time-token"])
```

### ServerAuthDescriptor

Source: `packages/contracts/src/auth.ts:174`.

```text
Object({
  policy: ServerAuthPolicy,
  bootstrapMethods: Array(ServerAuthBootstrapMethod),
  sessionMethods: Array(ServerAuthSessionMethod),
  sessionCookieName: TrimmedNonEmptyString,
  serverUpdateScope: Optional(Const(AuthEnvironmentMaintainScope)),
})
```

### ServerAuthPolicy

Source: `packages/contracts/src/auth.ts:12`.

```text
Enum([
  "desktop-managed-local",
  "loopback-browser",
  "remote-reachable",
  "unsafe-no-auth",
])
```

### ServerAuthSessionMethod

Source: `packages/contracts/src/auth.ts:23`.

```text
Enum([
  "browser-session-cookie",
  "bearer-access-token",
  "dpop-access-token",
])
```

### ServerConfig

Source: `packages/contracts/src/server.ts:582`.

```text
Object({
  environment: ExecutionEnvironmentDescriptor,
  auth: ServerAuthDescriptor,
  cwd: TrimmedNonEmptyString,
  keybindingsConfigPath: TrimmedNonEmptyString,
  keybindings: ResolvedKeybindingsConfig,
  issues: ServerConfigIssues,
  providers: ServerProviders,
  availableEditors: ForwardCompatibleArray(EditorId),
  remoteOpenTargets: Optional(ForwardCompatibleArray(RemoteOpenTarget)),
  directEndpoints: Optional(ForwardCompatibleArray(ServerDirectEndpoint)),
  observability: ServerObservability,
  settings: ServerSettings,
  shellResumeCompletionMarker: Optional(Boolean),
  shellThreadFamilies: Optional(Boolean),
  shellOlderSettled: Optional(Boolean),
  shellRevealInFileManager: Optional(Boolean),
  shellRevealInFileManagerKind: Optional(FileManagerRevealKind),
  threadResumeCompletionMarker: Optional(Boolean),
  threadSnapshotPagination: Optional(Boolean),
  reasoningMessages: Optional(Boolean),
  threadFind: Optional(Boolean),
  threadFindProgressive: Optional(Boolean),
  scratchWorkspaceRoot: Optional(TrimmedNonEmptyString),
  newProjectsRoot: Optional(TrimmedNonEmptyString),
  lastEventLoopStall: Optional(ServerEventLoopStall),
  environmentThemes: Optional(Array(EnvironmentTheme)),
  usageLimitSources: Optional(UsageLimitSourceSnapshots),
})
```

### ServerConfigEnvironmentThemesUpdatedPayload

Source: `packages/contracts/src/server.ts:724`.

```text
Object({
  themes: Array(EnvironmentTheme),
})
```

### ServerConfigIssue

Source: `packages/contracts/src/server.ts:43`.

```text
Union([
  KeybindingsMalformedConfigIssue,
  KeybindingsInvalidEntryIssue,
])
```

### ServerConfigIssues

Source: `packages/contracts/src/server.ts:49`.

```text
ForwardCompatibleArray(ServerConfigIssue)
```

### ServerConfigKeybindingsUpdatedPayload

Source: `packages/contracts/src/server.ts:676`.

```text
Object({
  keybindings: ResolvedKeybindingsConfig,
  issues: ServerConfigIssues,
})
```

### ServerConfigProviderStatusesPayload

Source: `packages/contracts/src/server.ts:683`.

```text
Object({
  providers: ServerProviders,
})
```

### ServerConfigSettingsUpdatedPayload

Source: `packages/contracts/src/server.ts:688`.

```text
Object({
  settings: ServerSettings,
})
```

### ServerConfigStreamEnvironmentThemesUpdatedEvent

Source: `packages/contracts/src/server.ts:730`.

```text
Object({
  version: Const(1),
  type: Const("environmentThemesUpdated"),
  payload: ServerConfigEnvironmentThemesUpdatedPayload,
})
```

### ServerConfigStreamEvent

Source: `packages/contracts/src/server.ts:752`.

```text
Union([
  ServerConfigStreamSnapshotEvent,
  ServerConfigStreamKeybindingsUpdatedEvent,
  ServerConfigStreamProviderStatusesEvent,
  ServerConfigStreamSettingsUpdatedEvent,
  ServerConfigStreamEnvironmentThemesUpdatedEvent,
  ServerConfigStreamUsageLimitSourcesUpdatedEvent,
])
```

### ServerConfigStreamKeybindingsUpdatedEvent

Source: `packages/contracts/src/server.ts:700`.

```text
Object({
  version: Const(1),
  type: Const("keybindingsUpdated"),
  payload: ServerConfigKeybindingsUpdatedPayload,
})
```

### ServerConfigStreamProviderStatusesEvent

Source: `packages/contracts/src/server.ts:708`.

```text
Object({
  version: Const(1),
  type: Const("providerStatuses"),
  payload: ServerConfigProviderStatusesPayload,
})
```

### ServerConfigStreamSettingsUpdatedEvent

Source: `packages/contracts/src/server.ts:716`.

```text
Object({
  version: Const(1),
  type: Const("settingsUpdated"),
  payload: ServerConfigSettingsUpdatedPayload,
})
```

### ServerConfigStreamSnapshotEvent

Source: `packages/contracts/src/server.ts:693`.

```text
Object({
  version: Const(1),
  type: Const("snapshot"),
  config: ServerConfig,
})
```

### ServerConfigStreamUsageLimitSourcesUpdatedEvent

Source: `packages/contracts/src/server.ts:744`.

```text
Object({
  version: Const(1),
  type: Const("usageLimitSourcesUpdated"),
  payload: ServerConfigUsageLimitSourcesUpdatedPayload,
})
```

### ServerConfigUsageLimitSourcesUpdatedPayload

Source: `packages/contracts/src/server.ts:738`.

```text
Object({
  sources: UsageLimitSourceSnapshots,
})
```

### ServerDirectEndpoint

Source: `packages/contracts/src/server.ts:563`.

```text
Object({
  kind: ServerDirectEndpointKind,
  httpBaseUrl: TrimmedNonEmptyString,
})
```

### ServerDirectEndpointKind

Source: `packages/contracts/src/server.ts:560`.

```text
Enum(["lan", "tailnet"])
```

### ServerEventLoopStall

Source: `packages/contracts/src/server.ts:573`.

```text
Object({
  detectedAt: IsoDateTime,
  durationMs: NonNegativeInt,
  loadAverage: Optional(Number),
  cpuCount: PositiveInt,
})
```

### ServerInstallation

Source: `packages/contracts/src/environment.ts:59`.

```text
Union([
  Object({ kind: Enum(["npx", "pnpm-dlx", "bunx"]) }),
  Object({ kind: Const("npm-global"), prefix: TrimmedNonEmptyString }),
])
```

### ServerLifecycleLegacyThreadMigrationPayload

Source: `packages/contracts/src/server.ts:790`.

```text
Object({
  status: Union([Const("running"), Const("complete")]),
  totalThreadCount: NonNegativeInt,
})
```

### ServerLifecycleReadyPayload

Source: `packages/contracts/src/server.ts:771`.

```text
Object({
  at: IsoDateTime,
  environment: ExecutionEnvironmentDescriptor,
  updateOutcome: Optional(ServerSelfUpdateOutcome),
})
```

### ServerLifecycleStreamEvent

Source: `packages/contracts/src/server.ts:822`.

```text
Union([
  ServerLifecycleStreamWelcomeEvent,
  ServerLifecycleStreamReadyEvent,
  ServerLifecycleStreamLegacyThreadMigrationEvent,
])
```

### ServerLifecycleStreamLegacyThreadMigrationEvent

Source: `packages/contracts/src/server.ts:813`.

```text
Object({
  version: Const(1),
  sequence: NonNegativeInt,
  type: Const("legacyThreadMigration"),
  payload: ServerLifecycleLegacyThreadMigrationPayload,
})
```

### ServerLifecycleStreamReadyEvent

Source: `packages/contracts/src/server.ts:805`.

```text
Object({
  version: Const(1),
  sequence: NonNegativeInt,
  type: Const("ready"),
  payload: ServerLifecycleReadyPayload,
})
```

### ServerLifecycleStreamWelcomeEvent

Source: `packages/contracts/src/server.ts:797`.

```text
Object({
  version: Const(1),
  sequence: NonNegativeInt,
  type: Const("welcome"),
  payload: ServerLifecycleWelcomePayload,
})
```

### ServerLifecycleWelcomePayload

Source: `packages/contracts/src/server.ts:778`.

```text
Object({
  environment: ExecutionEnvironmentDescriptor,
  cwd: TrimmedNonEmptyString,
  projectName: TrimmedNonEmptyString,
  bootstrapStatus: Optional(Enum(["pending", "complete"])),
  bootstrapProjectId: Optional(ProjectId),
  bootstrapThreadId: Optional(ThreadId),
  bootstrapProjectCreated: Optional(Boolean),
  bootstrapThreadCreated: Optional(Boolean),
})
```

### ServerObservability

Source: `packages/contracts/src/server.ts:283`.

```text
Object({
  logsDirectoryPath: TrimmedNonEmptyString,
  localTracingEnabled: Boolean,
  otlpTracesUrl: Optional(TrimmedNonEmptyString),
  otlpTracesSource: Optional(OtlpEndpointSource),
  otlpTracesVariable: Optional(TrimmedNonEmptyString),
  otlpTracesEnabled: Boolean,
  otlpMetricsUrl: Optional(TrimmedNonEmptyString),
  otlpMetricsSource: Optional(OtlpEndpointSource),
  otlpMetricsVariable: Optional(TrimmedNonEmptyString),
  otlpMetricsEnabled: Boolean,
  otlpLogsUrl: Optional(TrimmedNonEmptyString),
  otlpLogsSource: Optional(OtlpEndpointSource),
  otlpLogsVariable: Optional(TrimmedNonEmptyString),
  otlpLogsEnabled: Boolean,
})
```

### ServerProcessDiagnosticsEntry

Source: `packages/contracts/src/server.ts:403`.

```text
Object({
  pid: PositiveInt,
  startTimeMs: NonNegativeInt,
  ppid: NonNegativeInt,
  pgid: Option(Int),
  status: TrimmedNonEmptyString,
  cpuPercent: Number,
  rssBytes: NonNegativeInt,
  elapsed: TrimmedNonEmptyString,
  command: TrimmedNonEmptyString,
  depth: NonNegativeInt,
  childPids: Array(PositiveInt),
})
```

### ServerProcessDiagnosticsResult

Source: `packages/contracts/src/server.ts:418`.

```text
Object({
  serverPid: PositiveInt,
  readAt: DateTimeUtc,
  processCount: NonNegativeInt,
  totalRssBytes: NonNegativeInt,
  totalCpuPercent: Number,
  processes: Array(ServerProcessDiagnosticsEntry),
  error: Option(
    Object({
      message: TrimmedNonEmptyString,
    }),
})
```

### ServerProcessResourceHistoryBucket

Source: `packages/contracts/src/server.ts:439`.

```text
Object({
  startedAt: DateTimeUtc,
  endedAt: DateTimeUtc,
  avgCpuPercent: Number,
  maxCpuPercent: Number,
  maxRssBytes: NonNegativeInt,
  maxProcessCount: NonNegativeInt,
})
```

### ServerProcessResourceHistoryFailureTag

Source: `packages/contracts/src/server.ts:468`.

```text
Enum([
  "ProcessDiagnosticsQueryTimeoutError",
  "ProcessDiagnosticsQueryFailedError",
  "ProcessDiagnosticsServerProcessSignalError",
  "ProcessDiagnosticsNotDescendantError",
  "ProcessDiagnosticsSignalFailedError",
])
```

### ServerProcessResourceHistoryInput

Source: `packages/contracts/src/server.ts:433`.

```text
Object({
  windowMs: NonNegativeInt,
  bucketMs: NonNegativeInt,
})
```

### ServerProcessResourceHistoryResult

Source: `packages/contracts/src/server.ts:478`.

```text
Object({
  readAt: DateTimeUtc,
  windowMs: NonNegativeInt,
  bucketMs: NonNegativeInt,
  sampleIntervalMs: NonNegativeInt,
  retainedSampleCount: NonNegativeInt,
  totalCpuSecondsApprox: Number,
  buckets: Array(ServerProcessResourceHistoryBucket),
  topProcesses: Array(ServerProcessResourceHistorySummary),
  error: Option(
    Object({
      failureTag: ServerProcessResourceHistoryFailureTag,
      message: TrimmedNonEmptyString,
    }),
})
```

### ServerProcessResourceHistorySummary

Source: `packages/contracts/src/server.ts:449`.

```text
Object({
  processKey: TrimmedNonEmptyString,
  pid: PositiveInt,
  ppid: NonNegativeInt,
  command: TrimmedNonEmptyString,
  depth: NonNegativeInt,
  isServerRoot: Boolean,
  firstSeenAt: DateTimeUtc,
  lastSeenAt: DateTimeUtc,
  currentCpuPercent: Number,
  avgCpuPercent: Number,
  maxCpuPercent: Number,
  cpuSecondsApprox: Number,
  currentRssBytes: NonNegativeInt,
  maxRssBytes: NonNegativeInt,
  sampleCount: NonNegativeInt,
})
```

### ServerProcessSignal

Source: `packages/contracts/src/server.ts:400`.

```text
Enum(["SIGINT", "SIGKILL"])
```

### ServerProvider

Source: `packages/contracts/src/server.ts:205`.

```text
Object({
  instanceId: ProviderInstanceId,
  driver: ProviderDriverKind,
  displayName: Optional(TrimmedNonEmptyString),
  accentColor: Optional(TrimmedNonEmptyString),
  iconUrl: Optional(TrimmedNonEmptyString.check(isMaxLength(2_048))),
  badgeLabel: Optional(TrimmedNonEmptyString),
  continuation: Optional(ServerProviderContinuation),
  showInteractionModeToggle: Optional(Boolean),
  reportsContextWindow: Optional(Boolean),
  supportedRuntimeModes: Optional(ForwardCompatibleArray(RuntimeMode)),
  requiresNewThreadForModelChange: Optional(Boolean),
  supportsConversationRollback: Optional(Boolean),
  supportsTextGeneration: Optional(Boolean),
  setup: Optional(
    Object({
      canAuthenticate: Boolean,
      canInstall: Boolean,
      documentationUrl: Optional(TrimmedNonEmptyString.check(isMaxLength(2_048))),
    }),
  nativeSessions: Optional(
    Object({
      canList: Boolean,
      canLoad: Boolean,
      canResume: Boolean,
      canDelete: Optional(Boolean),
    }),
  configurableProviders: Optional(Boolean),
  runtimePaths: Optional(
    Object({
      homePath: TrimmedNonEmptyString,
      shadowHomePath: Nullable(TrimmedNonEmptyString),
    }),
  enabled: Boolean,
  installed: Boolean,
  version: Nullable(TrimmedNonEmptyString),
  installedVersion: Optional(Nullable(TrimmedNonEmptyString)),
  status: ServerProviderState,
  auth: ServerProviderAuth,
  checkedAt: IsoDateTime,
  message: Optional(TrimmedNonEmptyString),
  availability: Optional(ServerProviderAvailability),
  unavailableReason: Optional(TrimmedNonEmptyString),
  models: Array(ServerProviderModel),
  updateRequiredModels: Optional(Array(ServerProviderUpdateRequiredModel)),
  slashCommands: Array(ServerProviderSlashCommand),
  skills: Array(ServerProviderSkill),
  workspaceSnapshots: Optional(Array(ServerProviderWorkspaceSnapshot)),
  usageLimits: Optional(ServerProviderUsageLimits),
  versionAdvisory: Optional(ServerProviderVersionAdvisory),
  compatibilityAdvisory: Optional(ServerProviderCompatibilityAdvisory),
  updateState: Optional(ServerProviderUpdateState),
})
```

### ServerProviderAuth

Source: `packages/contracts/src/server.ts:61`.

```text
Object({
  status: ServerProviderAuthStatus,
  type: Optional(TrimmedNonEmptyString),
  label: Optional(TrimmedNonEmptyString),
  email: Optional(TrimmedNonEmptyString),
  action: Optional(AcpRegistryUrlAuthAction),
  canLogout: Optional(Boolean),
  subscriptionSharing: Optional(Boolean),
  profileId: Optional(TrimmedNonEmptyString),
  workspaceId: Optional(TrimmedNonEmptyString),
})
```

### ServerProviderAuthStatus

Source: `packages/contracts/src/server.ts:54`.

```text
Enum([
  "authenticated",
  "unauthenticated",
  "unknown",
])
```

### ServerProviderAvailability

Source: `packages/contracts/src/server.ts:143`.

```text
Enum(["available", "unavailable"])
```

### ServerProviderCompatibilityAdvisory

Source: `packages/contracts/src/server.ts:158`.

```text
Object({
  status: ServerProviderCompatibilityStatus,
  latestVersionStatus: Optional(ServerProviderCompatibilityStatus),
  message: Nullable(TrimmedNonEmptyString),
  recommendedVersion: Nullable(TrimmedNonEmptyString),
  recommendedRange: Nullable(TrimmedNonEmptyString),
})
```

### ServerProviderCompatibilityStatus

Source: `packages/contracts/src/server.ts:151`.

```text
Enum([
  "unknown",
  "supported",
  "graceful",
  "unsupported",
  "broken",
])
```

### ServerProviderContinuation

Source: `packages/contracts/src/server.ts:146`.

```text
Object({
  groupKey: TrimmedNonEmptyString,
})
```

### ServerProviderModel

Source: `packages/contracts/src/server.ts:74`.

```text
Object({
  slug: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString,
  shortName: Optional(TrimmedNonEmptyString),
  subProvider: Optional(TrimmedNonEmptyString),
  aliases: Optional(Array(TrimmedNonEmptyString)),
  badge: Optional(Const("new")),
  isCustom: Boolean,
  isDefault: Optional(Boolean),
  isLegacy: Optional(Boolean),
  capabilities: Nullable(ModelCapabilities),
})
```

### ServerProviderResetCredits

Source: `packages/contracts/src/providerUsageLimits.ts:23`.

```text
Object({
  availableCount: NonNegativeInt,
  nextExpiresAt: Optional(IsoDateTime),
  nextCreditId: Optional(TrimmedNonEmptyString),
})
```

### ServerProviderSkill

Source: `packages/contracts/src/server.ts:108`.

```text
Object({
  name: TrimmedNonEmptyString,
  description: Optional(TrimmedNonEmptyString),
  path: TrimmedNonEmptyString,
  scope: Optional(TrimmedNonEmptyString),
  enabled: Boolean,
  displayName: Optional(TrimmedNonEmptyString),
  shortDescription: Optional(TrimmedNonEmptyString),
  userInvocationOnly: Optional(Boolean),
  userInvocable: Optional(Boolean),
})
```

### ServerProviderSlashCommand

Source: `packages/contracts/src/server.ts:101`.

```text
Object({
  name: TrimmedNonEmptyString,
  description: Optional(TrimmedNonEmptyString),
  input: Optional(ServerProviderSlashCommandInput),
})
```

### ServerProviderSlashCommandInput

Source: `packages/contracts/src/server.ts:96`.

```text
Object({
  hint: TrimmedNonEmptyString,
})
```

### ServerProviderState

Source: `packages/contracts/src/server.ts:51`.

```text
Enum(["ready", "warning", "error", "disabled"])
```

### ServerProviderUpdateError

Source: `packages/contracts/src/server.ts:841`.

```text
Object({_tag: Const("ServerProviderUpdateError"),
    provider: ProviderDriverKind,
    reason: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ServerProviderUpdateInput

Source: `packages/contracts/src/server.ts:834`.

```text
Object({
  provider: ProviderDriverKind,
  targetVersion: Optional(TrimmedNonEmptyString),
  instanceId: Optional(ProviderInstanceId),
})
```

### ServerProviderUpdateRequiredModel

Source: `packages/contracts/src/server.ts:88`.

```text
Object({
  slug: TrimmedNonEmptyString,
  name: TrimmedNonEmptyString,
  badge: Optional(Const("new")),
  minVersion: TrimmedNonEmptyString,
})
```

### ServerProviderUpdateState

Source: `packages/contracts/src/server.ts:196`.

```text
Object({
  status: ServerProviderUpdateStatus,
  startedAt: Nullable(IsoDateTime),
  finishedAt: Nullable(IsoDateTime),
  message: Nullable(TrimmedNonEmptyString),
  output: Nullable(String.check(isMaxLength(10_000))),
})
```

### ServerProviderUpdateStatus

Source: `packages/contracts/src/server.ts:186`.

```text
Enum([
  "idle",
  "queued",
  "running",
  "succeeded",
  "failed",
  "unchanged",
])
```

### ServerProviderUpdatedPayload

Source: `packages/contracts/src/server.ts:829`.

```text
Object({
  providers: ServerProviders,
})
```

### ServerProviderUsageLimits

Source: `packages/contracts/src/providerUsageLimits.ts:32`.

```text
Object({
  checkedAt: IsoDateTime,
  windows: ForwardCompatibleArray(ServerProviderUsageWindow),
  credentialFingerprint: Optional(TrimmedNonEmptyString),
  resetCredits: Optional(ServerProviderResetCredits),
  externalUsage: Optional(
    Object({
      label: TrimmedNonEmptyString,
      url: TrimmedNonEmptyString,
    }),
  unavailable: Optional(
    Object({
      reason: Enum(["unsupported", "probeFailed"]),
      message: Optional(TrimmedNonEmptyString),
    }),
})
```

### ServerProviderUsageWindow

Source: `packages/contracts/src/providerUsageLimits.ts:13`.

```text
Object({
  id: TrimmedNonEmptyString,
  kind: Enum(["session", "weekly", "monthly", "other"]),
  label: TrimmedNonEmptyString,
  usedPercent: Number.check(isBetween({ minimum: 0, maximum: 100 }),
  resetsAt: Optional(IsoDateTime),
  windowDurationMins: Optional(NonNegativeInt),
})
```

### ServerProviderVersionAdvisory

Source: `packages/contracts/src/server.ts:174`.

```text
Object({
  status: ServerProviderVersionAdvisoryStatus,
  currentVersion: Nullable(TrimmedNonEmptyString),
  latestVersion: Nullable(TrimmedNonEmptyString),
  updateCommand: Nullable(TrimmedNonEmptyString),
  canUpdate: Boolean,
  canInstallVersion: Optional(Boolean),
  checkedAt: Nullable(IsoDateTime),
  message: Nullable(TrimmedNonEmptyString),
})
```

### ServerProviderVersionAdvisoryStatus

Source: `packages/contracts/src/server.ts:167`.

```text
Enum([
  "unknown",
  "current",
  "behind_latest",
])
```

### ServerProviderWorkspaceSnapshot

Source: `packages/contracts/src/server.ts:123`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  checkedAt: IsoDateTime,
  slashCommands: Array(ServerProviderSlashCommand),
  slashCommandsPending: Optional(Boolean),
  skills: Array(ServerProviderSkill),
  optionDescriptors: Optional(Array(ProviderOptionDescriptor)),
})
```

### ServerProviders

Source: `packages/contracts/src/server.ts:270`.

```text
ForwardCompatibleArray(ServerProvider)
```

### ServerSelfUpdateCapability

Source: `packages/contracts/src/environment.ts:65`.

```text
Enum([
  "boot-service",
  "respawn",
  "desktop-managed",
])
```

### ServerSelfUpdateError

Source: `packages/contracts/src/server.ts:889`.

```text
Object({_tag: Const("ServerSelfUpdateError"),
    reason: TrimmedNonEmptyString,
    cause: Optional(Defect()),
  })
```

### ServerSelfUpdateInput

Source: `packages/contracts/src/server.ts:854`.

```text
Object({
  targetVersion: TrimmedNonEmptyString,
  continueRunningThreads: Optional(Boolean),
})
```

### ServerSelfUpdateMethod

Source: `packages/contracts/src/environment.ts:56`.

```text
Enum(["boot-service", "respawn", "desktop-app"])
```

### ServerSelfUpdateOutcome

Source: `packages/contracts/src/server.ts:762`.

```text
Object({
  id: TrimmedNonEmptyString,
  fromVersion: TrimmedNonEmptyString,
  targetVersion: TrimmedNonEmptyString,
  status: Enum(["committed", "rolled-back", "failed"]),
  reason: Optional(TrimmedNonEmptyString),
})
```

### ServerSelfUpdateProgressEvent

Source: `packages/contracts/src/server.ts:877`.

```text
Union([
  Object({
    type: Const("progress"),
    stage: ServerSelfUpdateProgressStage,
  }),
  Object({
    type: Const("complete"),
    result: ServerSelfUpdateResult,
  }),
])
```

### ServerSelfUpdateProgressStage

Source: `packages/contracts/src/server.ts:874`.

```text
Enum(["downloading", "installing"])
```

### ServerSelfUpdateResult

Source: `packages/contracts/src/server.ts:861`.

```text
Object({
  targetVersion: TrimmedNonEmptyString,
  method: ServerSelfUpdateMethod,
  updateId: Optional(TrimmedNonEmptyString),
  desktopUpdateToken: Optional(TrimmedNonEmptyString),
})
```

### ServerSettings

Source: `packages/contracts/src/settings.ts:1103`.

```text
Object({
  favorites: Nullable(ModelFavorites),
  providerModelPreferences: Nullable(ProviderModelPreferences),
  worktreeCleanup: WorktreeCleanup,
  storageCleanup: StorageCleanupSettings,
  worktreesDirectory: TrimmedString,
  previousWorktreesDirectories: Array(TrimmedString),
  responseStreamingMode: ResponseStreamingMode,
  enableProviderUpdateChecks: Boolean,
  continueThreadsAfterServerUpdate: Boolean,
  enableAgentBrowserAccess: Boolean,
  projectAgentBrowserAccessOverrides: Record(ProjectId, Boolean),
  defaultAutoPull: Boolean,
  defaultProjectScripts: Array(ProjectScript),
  projectScriptOverrides: Record(ProjectId, Nullable(Array(ProjectScript))),
  projectAutoPullOverrides: Record(ProjectId, Boolean),
  defaultModelSelection: Nullable(ModelSelection),
  defaultRuntimeMode: RuntimeMode,
  projectSettingsOverrides: Record(ProjectId, ProjectSettingsOverrides),
  projectSettingsFolded: Boolean,
  enableAgentDeviceAccess: Boolean,
  enableDeviceSupport: Boolean,
  deviceOnboardingCompleted: Boolean,
  deviceHosts: SshDeviceHostConfigs,
  easDeviceHosts: EasDeviceHostConfigs,
  sidebarAutoSettleAfterDays: Nullable(SidebarAutoSettleAfterDays),
  compactIdleThreads: LegacyCompactIdleThreads,
  idleCompaction: Optional(IdleCompactionMode),
  snoozeLimitedThreads: Boolean,
  autoResumeLimitedThreads: Boolean,
  guestSharingEnabled: Boolean,
  sidebarAutoSettleKeepPullRequests: Boolean,
  sidebarAutoSettleOnMerge: Boolean,
  backgroundActivity: BackgroundActivitySettings,
  automaticGitFetchInterval: DurationFromMillis,
  providerHealthRefreshInterval: DurationFromMillis,
  backgroundActivityProfile: BackgroundActivityProfile,
  defaultTheme: DefaultThemePreference,
  defaultThemeSetAt: String.check(isMaxLength(64)),
  environmentIcon: ForwardCompatibleNullable(EnvironmentMachineKind),
  defaultThreadEnvMode: OmittedWhenNull(ThreadEnvMode),
  newWorktreesStartFromOrigin: Boolean,
  worktreeSubmodules: ForwardCompatibleNullable(WorktreeSubmodules),
  addProjectBaseDirectory: TrimmedString,
  textGenerationModelSelection: ModelSelection,
  threadTitleInstructions: TrimmedString,
  branchNamingMode: BranchNamingMode,
  branchNamePrefix: TrimmedString,
  branchNameInstructions: TrimmedString,
  removeAgentCreditsOnMerge: Boolean,
  sourceControlWritingStyle: SourceControlWritingStyleSetting,
  sourceControlWriterModelSelection: Nullable(ModelSelection),
  reviewGuidesEnabled: Boolean,
  reviewGuideModelSelection: Nullable(ModelSelection),
  reviewGuideInstructions: TrimmedString,
  pullRequestMergeMethod: Nullable(PullRequestMergeMethod),
  providerInstances: Record(ProviderInstanceId, ProviderInstanceConfig),
  observability: ObservabilitySettings,
  sourceControlHosts: Record(String, SourceControlHostSettings),
  usageLimitSources: Record(UsageLimitSourceId, UsageLimitSourceConfig),
  cursorKeychainUsageEnabled: Boolean,
  usagePriceOverrides: Record(TrimmedNonEmptyString, UsageModelPriceOverride),
  usageModelAliases: Record(TrimmedNonEmptyString, TrimmedNonEmptyString),
})
```

### ServerSettingsError

Source: `packages/contracts/src/settings.ts:1391`.

```text
Object({_tag: Const("ServerSettingsError"),
    settingsPath: String,
    operation: ServerSettingsOperation,
    providerInstanceId: Optional(String),
    environmentVariable: Optional(String),
    cause: Optional(Defect()),
  })
```

### ServerSettingsOperation

Source: `packages/contracts/src/settings.ts:1375`.

```text
Enum([
  "normalize",
  "check-exists",
  "create-provider-instance",
  "read-file",
  "read-provider-history",
  "read-project-settings",
  "read-secret",
  "remove-secret",
  "remove-stale-secret",
  "write-secret",
  "write-file",
  "prepare-directory",
])
```

### ServerSettingsPatch

Source: `packages/contracts/src/settings.ts:1424`.

```text
Object({
  setModelFavorites: Optional(
    Array(Object({ ...ModelFavorite.fields, favorite: Boolean }),
  ),
  setModelsHidden: Optional(
    Array(Object({ ...ModelFavorite.fields, hidden: Boolean }),
  ),
  setProviderModelOrder: Optional(
    Object({ provider: ProviderInstanceId, modelOrder: Array(TrimmedString) }),
  favorites: Optional(ModelFavorites),
  providerModelPreferences: Optional(ProviderModelPreferences),
  migrateModelPreferences: Optional(
    Object({
      favorites: Optional(ModelFavorites),
      providerModelPreferences: Optional(ProviderModelPreferences),
    }),
  worktreeCleanup: Optional(
    Nullable(
      Union([
        Object({ mode: Const("off") }),
        Object({
          mode: Const("custom"),
          rules: Object({
            worktreeKeepWhen: Optional(WorktreeKeepWhen),
            worktreeAfterDays: Optional(StorageRetentionDays),
            worktreeSettledAfterDays: Optional(SettledWorktreeRetentionDays),
            worktreeOnMerge: Optional(Boolean),
            worktreeOnDelete: Optional(Boolean),
            worktreeUnchanged: Optional(Boolean),
          }),
        }),
      ]),
    ),
  ),
  storageCleanup: Optional(
    Object({
      worktreeKeepWhen: Optional(WorktreeKeepWhen),
      worktreeAfterDays: Optional(StorageRetentionDays),
      worktreeSettledAfterDays: Optional(SettledWorktreeRetentionDays),
      worktreeOnMerge: Optional(Boolean),
      worktreeOnDelete: Optional(Boolean),
      worktreeUnchanged: Optional(Boolean),
      browserArtifactsAfterDays: Optional(StorageRetentionDays),
      logsAfterDays: Optional(StorageRetentionDays),
    }),
  worktreesDirectory: Optional(TrimmedString),
  responseStreamingMode: Optional(ResponseStreamingMode),
  enableProviderUpdateChecks: Optional(Boolean),
  continueThreadsAfterServerUpdate: Optional(Boolean),
  enableAgentBrowserAccess: Optional(Boolean),
  projectAgentBrowserAccessOverrides: Optional(
    Record(ProjectId, Nullable(Boolean)),
  ),
  defaultAutoPull: Optional(Boolean),
  defaultProjectScripts: Optional(Array(ProjectScript)),
  projectScriptOverrides: Optional(
    Record(ProjectId, Nullable(Array(ProjectScript))),
  ),
  projectAutoPullOverrides: Optional(
    Record(ProjectId, Nullable(Boolean)),
  ),
  defaultModelSelection: Optional(Nullable(ModelSelection)),
  defaultRuntimeMode: Optional(RuntimeMode),
  projectSettingsOverrides: Optional(
    Record(
      ProjectId,
      Nullable(
        Object({
          ...ProjectSettingsOverrides.fields,
          sourceControlWritingStyle: Optional(SourceControlWritingStyleSettings),
        }),
    ),
  ),
  enableAgentDeviceAccess: Optional(Boolean),
  enableDeviceSupport: Optional(Boolean),
  deviceOnboardingCompleted: Optional(Boolean),
  deviceHosts: Optional(SshDeviceHostConfigs),
  easDeviceHosts: Optional(EasDeviceHostConfigs),
  sidebarAutoSettleAfterDays: Optional(Nullable(SidebarAutoSettleAfterDays)),
  compactIdleThreads: Optional(LegacyCompactIdleThreads),
  idleCompaction: Optional(IdleCompactionMode),
  sidebarAutoSettleKeepPullRequests: Optional(Boolean),
  sidebarAutoSettleOnMerge: Optional(Boolean),
  autoResumeLimitedThreads: Optional(Boolean),
  snoozeLimitedThreads: Optional(Boolean),
  guestSharingEnabled: Optional(Boolean),
  backgroundActivity: Optional(
    Object({
      schemaVersion: Optional(Const(1)),
      profile: Optional(BackgroundActivityProfileSelection),
      baseProfile: Optional(BackgroundActivityProfile),
      overrides: Optional(BackgroundActivityOverrides),
    }),
  automaticGitFetchInterval: Optional(DurationFromMillis),
  providerHealthRefreshInterval: Optional(DurationFromMillis),
  backgroundActivityProfile: Optional(BackgroundActivityProfile),
  environmentIcon: Optional(Nullable(EnvironmentMachineKind)),
  defaultThreadEnvMode: Optional(Nullable(ThreadEnvMode)),
  newWorktreesStartFromOrigin: Optional(Boolean),
  worktreeSubmodules: Optional(Nullable(WorktreeSubmodules)),
  addProjectBaseDirectory: Optional(TrimmedString),
  textGenerationModelSelection: Optional(ModelSelectionPatch),
  threadTitleInstructions: Optional(TrimmedString),
  branchNamingMode: Optional(BranchNamingMode),
  branchNamePrefix: Optional(TrimmedString),
  branchNameInstructions: Optional(TrimmedString),
  removeAgentCreditsOnMerge: Optional(Boolean),
  sourceControlWritingStyle: Optional(
    Object({
      mode: Optional(SourceControlWritingStyleMode),
      customInstructions: Optional(TrimmedString),
      followChangeRequestTemplates: Optional(Boolean),
    }),
  sourceControlWriterModelSelection: Optional(Nullable(ModelSelection)),
  reviewGuidesEnabled: Optional(Boolean),
  reviewGuideModelSelection: Optional(Nullable(ModelSelection)),
  reviewGuideInstructions: Optional(TrimmedString),
  pullRequestMergeMethod: Optional(Nullable(PullRequestMergeMethod)),
  observability: Optional(
    Object({
      otlpTracesUrl: Optional(TrimmedString),
      otlpMetricsUrl: Optional(TrimmedString),
      otlpLogsUrl: Optional(TrimmedString),
    }),
  sourceControlHosts: Optional(Record(String, SourceControlHostSettings)),
  providerInstances: Optional(Record(ProviderInstanceId, ProviderInstanceConfig)),
  usageLimitSources: Optional(
    Record(UsageLimitSourceId, Nullable(UsageLimitSourceConfig)),
  ),
  cursorKeychainUsageEnabled: Optional(Boolean),
  usagePriceOverrides: Optional(
    Record(TrimmedNonEmptyString, Nullable(UsageModelPriceOverride)),
  ),
  usageModelAliases: Optional(
    Record(TrimmedNonEmptyString, Nullable(TrimmedNonEmptyString)),
  ),
})
```

### ServerSignalProcessInput

Source: `packages/contracts/src/server.ts:496`.

```text
Object({
  pid: PositiveInt,
  startTimeMs: NonNegativeInt,
  signal: ServerProcessSignal,
})
```

### ServerSignalProcessResult

Source: `packages/contracts/src/server.ts:503`.

```text
Object({
  pid: PositiveInt,
  signal: ServerProcessSignal,
  signaled: Boolean,
  message: Option(TrimmedNonEmptyString),
})
```

### ServerTraceDiagnosticsErrorKind

Source: `packages/contracts/src/server.ts:317`.

```text
Enum([
  "trace-file-not-found",
  "trace-file-read-failed",
])
```

### ServerTraceDiagnosticsFailureSummary

Source: `packages/contracts/src/server.ts:333`.

```text
Object({
  name: TrimmedNonEmptyString,
  cause: TrimmedNonEmptyString,
  count: NonNegativeInt,
  lastSeenAt: DateTimeUtc,
  traceId: TrimmedNonEmptyString,
  spanId: TrimmedNonEmptyString,
})
```

### ServerTraceDiagnosticsLogEvent

Source: `packages/contracts/src/server.ts:362`.

```text
Object({
  spanName: TrimmedNonEmptyString,
  level: TrimmedNonEmptyString,
  message: TrimmedNonEmptyString,
  seenAt: DateTimeUtc,
  traceId: TrimmedNonEmptyString,
  spanId: TrimmedNonEmptyString,
})
```

### ServerTraceDiagnosticsRecentFailure

Source: `packages/contracts/src/server.ts:343`.

```text
Object({
  name: TrimmedNonEmptyString,
  cause: TrimmedNonEmptyString,
  durationMs: Number,
  endedAt: DateTimeUtc,
  traceId: TrimmedNonEmptyString,
  spanId: TrimmedNonEmptyString,
})
```

### ServerTraceDiagnosticsResult

Source: `packages/contracts/src/server.ts:372`.

```text
Object({
  traceFilePath: TrimmedNonEmptyString,
  scannedFilePaths: Array(TrimmedNonEmptyString),
  readAt: DateTimeUtc,
  recordCount: NonNegativeInt,
  parseErrorCount: NonNegativeInt,
  firstSpanAt: Option(DateTimeUtc),
  lastSpanAt: Option(DateTimeUtc),
  failureCount: NonNegativeInt,
  interruptionCount: NonNegativeInt,
  slowSpanThresholdMs: NonNegativeInt,
  slowSpanCount: NonNegativeInt,
  logLevelCounts: Record(TrimmedNonEmptyString, NonNegativeInt),
  topSpansByCount: Array(ServerTraceDiagnosticsSpanSummary),
  slowestSpans: Array(ServerTraceDiagnosticsSpanOccurrence),
  commonFailures: Array(ServerTraceDiagnosticsFailureSummary),
  latestFailures: Array(ServerTraceDiagnosticsRecentFailure),
  latestWarningAndErrorLogs: Array(ServerTraceDiagnosticsLogEvent),
  partialFailure: Option(Boolean),
  error: Option(
    Object({
      kind: ServerTraceDiagnosticsErrorKind,
      message: TrimmedNonEmptyString,
    }),
})
```

### ServerTraceDiagnosticsSpanOccurrence

Source: `packages/contracts/src/server.ts:353`.

```text
Object({
  name: TrimmedNonEmptyString,
  durationMs: Number,
  endedAt: DateTimeUtc,
  traceId: TrimmedNonEmptyString,
  spanId: TrimmedNonEmptyString,
})
```

### ServerTraceDiagnosticsSpanSummary

Source: `packages/contracts/src/server.ts:323`.

```text
Object({
  name: TrimmedNonEmptyString,
  count: NonNegativeInt,
  failureCount: NonNegativeInt,
  totalDurationMs: Number,
  averageDurationMs: Number,
  maxDurationMs: Number,
})
```

### ServerUpsertKeybindingInput

Source: `packages/contracts/src/server.ts:649`.

```text
Object({
  key: KeybindingValue,
  command: KeybindingCommand,
  when: Optional(KeybindingWhen),
  replace: Optional(ServerUpsertKeybindingReplaceTarget),
})
```

### ServerUpsertKeybindingReplaceTarget

Source: `packages/contracts/src/server.ts:643`.

```text
Object({
  key: KeybindingValue,
  command: KeybindingCommand,
  when: Optional(KeybindingWhen),
})
```

### ServerUpsertKeybindingResult

Source: `packages/contracts/src/server.ts:660`.

```text
Object({
  keybindings: ResolvedKeybindingsConfig,
  issues: ServerConfigIssues,
})
```

### SettledWorktreeRetentionDays

Source: `packages/contracts/src/settings.ts:922`.

```text
Nullable(
  Int.check(isBetween({ minimum: 0, maximum: 3650 }),
)
```

### SetupOperationId

Source: `packages/contracts/src/providerSetup.ts:33`.

```text
TrimmedNonEmptyString.check(isMaxLength(128))
```

### SidebarAutoSettleAfterDays

Source: `packages/contracts/src/settings.ts:87`.

```text
Number.check(
  isBetween({
    minimum: MIN_SIDEBAR_AUTO_SETTLE_AFTER_DAYS,
    maximum: MAX_SIDEBAR_AUTO_SETTLE_AFTER_DAYS,
  })
```

### SkillContextRecord

Source: `packages/contracts/src/composerContext.ts:198`.

```text
Object({
  ...recordBase,
  kind: Const("skill"),
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
})
```

### SnapShotAccessibility

Source: `packages/contracts/src/chatAttachment.ts:108`.

```text
SnapShotAccessibilityWire.check(
  Predicate('flat-text')
```

### SnapShotAccessibilityBounds

Source: `packages/contracts/src/chatAttachment.ts:44`.

```text
Object({
  x: NonNegativeInt,
  y: NonNegativeInt,
  width: PositiveInt,
  height: PositiveInt,
})
```

### SnapShotAccessibilityNode

Source: `packages/contracts/src/chatAttachment.ts:74`.

```text
Object({
  role: TrimmedNonEmptyString.check(isMaxLength(100)),
  name: Optional(TrimmedNonEmptyString.check(isMaxLength(1_000))),
  value: Optional(TrimmedNonEmptyString.check(isMaxLength(8_000))),
  description: Optional(TrimmedNonEmptyString.check(isMaxLength(2_000))),
  bounds: Nullable(SnapShotAccessibilityBounds),
  state: Optional(SnapShotAccessibilityState),
  actions: Optional(
    mutable(Array(TrimmedNonEmptyString.check(isMaxLength(100)))).check(
      isMaxLength(32),
    ),
  ),
  children: mutable(
    Array(
      Ref(SnapShotAccessibilityNode),
    ),
  ).check(isMaxLength(SNAP_SHOT_ACCESSIBILITY_MAX_NODES)),
})
```

### SnapShotAccessibilityState

Source: `packages/contracts/src/chatAttachment.ts:51`.

```text
Object({
  active: Optional(Boolean),
  busy: Optional(Boolean),
  checked: Optional(Enum(["on", "off", "mixed"])),
  editable: Optional(Boolean),
  enabled: Optional(Boolean),
  expanded: Optional(Boolean),
  focused: Optional(Boolean),
  selected: Optional(Boolean),
  visible: Optional(Boolean),
})
```

### SnapShotAccessibilityWire

Source: `packages/contracts/src/chatAttachment.ts:93`.

```text
Union([
  Object({
    format: Const("flat-text"),
    text: TrimmedNonEmptyString.check(isMaxLength(SNAP_SHOT_ACCESSIBLE_TEXT_MAX_CHARS)),
    truncated: Boolean,
  }),
  Object({
    format: Const("element-tree"),
    coordinateSpace: Const("captured-image"),
    imageSize: Object({ width: PositiveInt, height: PositiveInt }),
    truncated: Boolean,
    root: SnapShotAccessibilityNode,
  }),
])
```

### SnapShotSource

Source: `packages/contracts/src/chatAttachment.ts:129`.

```text
Object({
  kind: Const("snap-shot"),
  capturedAt: IsoDateTime,
  appName: TrimmedNonEmptyString.check(isMaxLength(255)),
  windowTitle: TrimmedString.check(isMaxLength(1_000)),
  accessibleText: Optional(
    TrimmedNonEmptyString.check(isMaxLength(SNAP_SHOT_ACCESSIBLE_TEXT_MAX_CHARS)),
  ),
  accessibility: Optional(SnapShotAccessibility),
  appIdentifier: Optional(TrimmedNonEmptyString.check(isMaxLength(255))),
  appIconDataUrl: Optional(
    TrimmedNonEmptyString.check(
      isMaxLength(100_000),
      isPattern(/^data:image\/png;base64,/i),
    ),
  ),
})
```

### SourceControlCloneProtocol

Source: `packages/contracts/src/sourceControl.ts:51`.

```text
Enum(["auto", "ssh", "https"])
```

### SourceControlCloneRepositoryInput

Source: `packages/contracts/src/sourceControl.ts:69`.

```text
Object({
  provider: Optional(SourceControlProviderKind),
  repository: Optional(TrimmedNonEmptyString),
  remoteUrl: Optional(TrimmedNonEmptyString),
  destinationPath: TrimmedNonEmptyString,
  protocol: Optional(SourceControlCloneProtocol),
})
```

### SourceControlCloneRepositoryResult

Source: `packages/contracts/src/sourceControl.ts:78`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  remoteUrl: TrimmedNonEmptyString,
  repository: Nullable(SourceControlRepositoryInfo),
})
```

### SourceControlDiscoveryResult

Source: `packages/contracts/src/sourceControl.ts:162`.

```text
Object({
  versionControlSystems: Array(VcsDiscoveryItem),
  sourceControlProviders: Array(SourceControlProviderDiscoveryItem),
})
```

### SourceControlDiscoverySharedFields

Source: `packages/contracts/src/sourceControl.ts:139`.

```text
{
  label: TrimmedNonEmptyString,
  executable: Optional(TrimmedNonEmptyString),
  status: SourceControlDiscoveryStatus,
  version: Option(TrimmedNonEmptyString),
  installHint: TrimmedNonEmptyString,
  detail: Option(TrimmedNonEmptyString),
}
```

### SourceControlDiscoveryStatus

Source: `packages/contracts/src/sourceControl.ts:108`.

```text
Enum(["available", "missing"])
```

### SourceControlHostSettings

Source: `packages/contracts/src/settings.ts:816`.

```text
Record(String, Unknown)
```

### SourceControlProviderAuth

Source: `packages/contracts/src/sourceControl.ts:118`.

```text
Object({
  status: SourceControlProviderAuthStatus,
  account: Option(TrimmedNonEmptyString),
  host: Option(TrimmedNonEmptyString),
  detail: Option(TrimmedNonEmptyString),
  accounts: Optional(
    Array(
      Object({
        host: TrimmedNonEmptyString,
        account: TrimmedNonEmptyString,
        active: Boolean,
        authenticated: Boolean,
        error: Optional(TrimmedNonEmptyString),
        environmentVariable: Optional(TrimmedNonEmptyString),
      }),
  ),
})
```

### SourceControlProviderAuthStatus

Source: `packages/contracts/src/sourceControl.ts:111`.

```text
Enum([
  "authenticated",
  "unauthenticated",
  "unknown",
])
```

### SourceControlProviderDiscoveryItem

Source: `packages/contracts/src/sourceControl.ts:155`.

```text
Object({
  kind: SourceControlProviderKind,
  ...SourceControlDiscoverySharedFields,
  auth: SourceControlProviderAuth,
})
```

### SourceControlProviderError

Source: `packages/contracts/src/sourceControl.ts:168`.

```text
Object({_tag: Const("SourceControlProviderError"),
    provider: SourceControlProviderKind,
    operation: String,
    cwd: String,
    command: Optional(String),
    repository: Optional(String),
    reference: Optional(String),
    detail: String,
    cause: Optional(Defect()),
  })
```

### SourceControlProviderInfo

Source: `packages/contracts/src/sourceControl.ts:12`.

```text
Object({
  kind: SourceControlProviderKind,
  name: TrimmedNonEmptyString,
  baseUrl: String,
})
```

### SourceControlProviderKind

Source: `packages/contracts/src/sourceControl.ts:6`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(64),
  isPattern(/^[a-z][a-z0-9-]*$/),
)
```

### SourceControlPublishRepositoryInput

Source: `packages/contracts/src/sourceControl.ts:85`.

```text
Object({
  cwd: TrimmedNonEmptyString,
  provider: SourceControlProviderKind,
  repository: TrimmedNonEmptyString,
  visibility: SourceControlRepositoryVisibility,
  remoteName: Optional(TrimmedNonEmptyString),
  protocol: Optional(SourceControlCloneProtocol),
})
```

### SourceControlPublishRepositoryResult

Source: `packages/contracts/src/sourceControl.ts:98`.

```text
Object({
  repository: SourceControlRepositoryInfo,
  remoteName: TrimmedNonEmptyString,
  remoteUrl: TrimmedNonEmptyString,
  branch: TrimmedNonEmptyString,
  upstreamBranch: Optional(TrimmedNonEmptyString),
  status: SourceControlPublishStatus,
})
```

### SourceControlPublishStatus

Source: `packages/contracts/src/sourceControl.ts:95`.

```text
Enum(["pushed", "remote_added"])
```

### SourceControlRepositoryError

Source: `packages/contracts/src/sourceControl.ts:186`.

```text
Object({_tag: Const("SourceControlRepositoryError"),
    provider: SourceControlProviderKind,
    operation: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### SourceControlRepositoryInfo

Source: `packages/contracts/src/sourceControl.ts:54`.

```text
Object({
  provider: SourceControlProviderKind,
  nameWithOwner: TrimmedNonEmptyString,
  url: TrimmedNonEmptyString,
  sshUrl: TrimmedNonEmptyString,
})
```

### SourceControlRepositoryLookupInput

Source: `packages/contracts/src/sourceControl.ts:62`.

```text
Object({
  provider: SourceControlProviderKind,
  repository: TrimmedNonEmptyString,
  cwd: Optional(TrimmedNonEmptyString),
})
```

### SourceControlRepositoryVisibility

Source: `packages/contracts/src/sourceControl.ts:48`.

```text
Enum(["private", "public"])
```

### SourceControlWritingStyleMode

Source: `packages/contracts/src/settings.ts:826`.

```text
Enum([
  "repo_conventions",
  "conventional_commits",
  "custom",
])
```

### SourceControlWritingStyleSetting

Source: `packages/contracts/src/settings.ts:860`.

```text
Union([
  SourceControlWritingStyleSettings,
  LegacySourceControlWritingStyle,
])
```

### SourceControlWritingStyleSettings

Source: `packages/contracts/src/settings.ts:833`.

```text
Object({
  mode: SourceControlWritingStyleMode,
  customInstructions: TrimmedString,
  followChangeRequestTemplates: Boolean,
})
```

### SshDeviceHostConfig

Source: `packages/contracts/src/device.ts:21`.

```text
Object({
  kind: Const("ssh"),
  id: ConfiguredDeviceHostId,
  label: TrimmedNonEmptyString,
  target: TrimmedNonEmptyString.check(isPattern(/^[^\s-][^\s]*$/)),
  identityFile: Optional(TrimmedNonEmptyString),
  port: Optional(Int.check(isBetween({ minimum: 1, maximum: 65535 })),
})
```

### SshDeviceHostConfigs

Source: `packages/contracts/src/device.ts:79`.

```text
Array(SshDeviceHostConfig).check(uniqueHostIds)
```

### StorageCleanupReport

Source: `packages/contracts/src/settings.ts:1074`.

```text
Object({
  trigger: Enum(["automatic", "manual"]),
  startedAt: String,
  finishedAt: String,
  entries: Array(StorageCleanupReportEntry),
  counts: Object({ removed: Number, kept: Number, failed: Number }),
  omittedCount: Number,
  bytesFreed: Number,
})
```

### StorageCleanupReportEntry

Source: `packages/contracts/src/settings.ts:1062`.

```text
Object({
  kind: Enum(["worktree", "browser-artifacts", "logs"]),
  outcome: Enum(["removed", "kept", "failed"]),
  reason: String,
  path: Nullable(String),
  projectPath: Nullable(String),
  threadId: Nullable(ThreadId),
  threadTitle: Nullable(String),
  bytes: Nullable(Number),
  files: Nullable(Number),
})
```

### StorageCleanupSettings

Source: `packages/contracts/src/settings.ts:1085`.

```text
Object({
  worktreeKeepWhen: WorktreeKeepWhen,
  worktreeAfterDays: StorageRetentionDays,
  worktreeSettledAfterDays: SettledWorktreeRetentionDays,
  worktreeOnMerge: Boolean,
  worktreeOnDelete: Boolean,
  worktreeUnchanged: Boolean,
  browserArtifactsAfterDays: StorageRetentionDays,
  logsAfterDays: StorageRetentionDays,
})
```

### StorageRetentionDays

Source: `packages/contracts/src/settings.ts:919`.

```text
Nullable(
  Int.check(isBetween({ minimum: 1, maximum: 3650 }),
)
```

### StoredPreviewViewportPresetId

Source: `packages/contracts/src/preview.ts:82`.

```text
Enum([
  ...PREVIEW_VIEWPORT_PRESET_IDS,
  ...LEGACY_PREVIEW_VIEWPORT_PRESET_IDS,
])
```

### SubagentNotificationSource

Source: `packages/contracts/src/orchestrationV2.ts:1138`.

```text
Object({
  kind: Const("subagent"),
  childThreadId: Optional(ThreadId),
})
```

### THREAD_JUMP_KEYBINDING_COMMANDS

Source: `packages/contracts/src/keybindings.ts:10`.

```text
[
  "thread.jump.1",
  "thread.jump.2",
  "thread.jump.3",
  "thread.jump.4",
  "thread.jump.5",
  "thread.jump.6",
  "thread.jump.7",
  "thread.jump.8",
  "thread.jump.9",
]
```

### THREAD_KEYBINDING_COMMANDS

Source: `packages/contracts/src/keybindings.ts:37`.

```text
[
  "thread.stop",
  "thread.steerQueuedMessage",
  "thread.editQueuedMessage",
  "thread.previous",
  "thread.next",
  "thread.copyReference",
  "thread.settle",
  "thread.pin",
  "thread.undo",
  ...THREAD_JUMP_KEYBINDING_COMMANDS,
]
```

### THREAD_SEARCH_QUERY_MAX_LENGTH

Source: `packages/contracts/src/threadSearch.ts:16`.

```text
200
```

### TerminalActivityEvent

Source: `packages/contracts/src/terminal.ts:202`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("activity"),
  hasRunningSubprocess: Boolean,
  label: String.check(isMaxLength(128)),
})
```

### TerminalAttachInput

Source: `packages/contracts/src/terminal.ts:47`.

```text
Object({
  ...TerminalSessionInput.fields,
  cwd: Optional(TrimmedNonEmptyStringSchema),
  worktreePath: Optional(Nullable(TrimmedNonEmptyStringSchema)),
  cols: Optional(TerminalColsSchema),
  rows: Optional(TerminalRowsSchema),
  env: Optional(TerminalEnvSchema),
  providerInstanceId: Optional(ProviderInstanceId),
  restartIfNotRunning: Optional(Boolean),
  resume: Optional(Boolean),
})
```

### TerminalAttachSnapshotEvent

Source: `packages/contracts/src/terminal.ts:221`.

```text
Object({
  type: Const("snapshot"),
  snapshot: TerminalSessionSnapshot,
})
```

### TerminalAttachStreamEvent

Source: `packages/contracts/src/terminal.ts:226`.

```text
Union([
  TerminalAttachSnapshotEvent,
  TerminalOutputEvent,
  TerminalExitedEvent,
  TerminalClosedEvent,
  TerminalErrorEvent,
  TerminalClearedEvent,
  TerminalRestartedEvent,
  TerminalActivityEvent,
])
```

### TerminalClearedEvent

Source: `packages/contracts/src/terminal.ts:191`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("cleared"),
})
```

### TerminalCloseInput

Source: `packages/contracts/src/terminal.ts:91`.

```text
Object({
  ...TerminalThreadInput.fields,
  terminalId: Optional(TerminalIdSchema),
  deleteHistory: Optional(Boolean),
})
```

### TerminalClosedEvent

Source: `packages/contracts/src/terminal.ts:180`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("closed"),
})
```

### TerminalColsSchema

Source: `packages/contracts/src/terminal.ts:9`.

```text
Int.check(isGreaterThanOrEqualTo(1)).check(
  isLessThanOrEqualTo(1000),
)
```

### TerminalContextRecord

Source: `packages/contracts/src/composerContext.ts:108`.

```text
Object({
  ...recordBase,
  kind: Const("terminal"),
  terminalId: TrimmedNonEmptyString.check(isMaxLength(255)),
  terminalLabel: TrimmedNonEmptyString.check(isMaxLength(255)),
  lineStart: NonNegativeInt,
  lineEnd: NonNegativeInt,
  text: BoundedString(COMPOSER_CONTEXT_TERMINAL_TEXT_MAX_CHARS),
}).check(Predicate('semantic validation; see constraints in sections 2, 5 and 7'))
```

### TerminalCwdError

Source: `packages/contracts/src/terminal.ts:272`.

```text
Union([
  TerminalCwdNotFoundError,
  TerminalCwdNotDirectoryError,
  TerminalCwdStatError,
])
```

### TerminalCwdNotDirectoryError

Source: `packages/contracts/src/terminal.ts:249`.

```text
Object({_tag: Const("TerminalCwdNotDirectoryError"),
    cwd: String,
  })
```

### TerminalCwdNotFoundError

Source: `packages/contracts/src/terminal.ts:238`.

```text
Object({_tag: Const("TerminalCwdNotFoundError"),
    cwd: String,
  })
```

### TerminalCwdStatError

Source: `packages/contracts/src/terminal.ts:260`.

```text
Object({_tag: Const("TerminalCwdStatError"),
    cwd: String,
    cause: Defect(),
  })
```

### TerminalEnvKeySchema

Source: `packages/contracts/src/terminal.ts:16`.

```text
String.check(
  isPattern(/^[A-Za-z_][A-Za-z0-9_]*$/),
).check(isMaxLength(128))
```

### TerminalEnvSchema

Source: `packages/contracts/src/terminal.ts:20`.

```text
Record(TerminalEnvKeySchema, TerminalEnvValueSchema).check(
  isMaxProperties(128),
)
```

### TerminalEnvValueSchema

Source: `packages/contracts/src/terminal.ts:19`.

```text
String.check(isMaxLength(8_192))
```

### TerminalError

Source: `packages/contracts/src/terminal.ts:370`.

```text
Union([
  TerminalCwdError,
  TerminalHistoryError,
  TerminalSessionLookupError,
  TerminalProviderInstanceNotFoundError,
  TerminalProviderEnvironmentError,
  TerminalNotRunningError,
  TerminalWriteError,
  TerminalResizeError,
])
```

### TerminalErrorEvent

Source: `packages/contracts/src/terminal.ts:185`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("error"),
  message: String.check(isNonEmpty()),
})
```

### TerminalEvent

Source: `packages/contracts/src/terminal.ts:209`.

```text
Union([
  TerminalStartedEvent,
  TerminalOutputEvent,
  TerminalExitedEvent,
  TerminalClosedEvent,
  TerminalErrorEvent,
  TerminalClearedEvent,
  TerminalRestartedEvent,
  TerminalActivityEvent,
])
```

### TerminalEventBaseSchema

Source: `packages/contracts/src/terminal.ts:155`.

```text
Object({
  threadId: String.check(isNonEmpty()),
  terminalId: String.check(isNonEmpty()),
  sequence: Optional(Int.check(isGreaterThanOrEqualTo(0))),
})
```

### TerminalExitedEvent

Source: `packages/contracts/src/terminal.ts:173`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("exited"),
  exitCode: Nullable(Int),
  exitSignal: Nullable(Int),
})
```

### TerminalHistoryError

Source: `packages/contracts/src/terminal.ts:279`.

```text
Object({_tag: Const("TerminalHistoryError"),
    operation: Enum(["read", "truncate", "migrate"]),
    threadId: String,
    terminalId: String,
    cause: Optional(Defect()),
  })
```

### TerminalIdSchema

Source: `packages/contracts/src/terminal.ts:15`.

```text
TrimmedNonEmptyStringcheck(isMaxLength(128))
```

### TerminalMetadataRemoveEvent

Source: `packages/contracts/src/terminal.ts:142`.

```text
Object({
  type: Const("remove"),
  threadId: String.check(isNonEmpty()),
  terminalId: String.check(isNonEmpty()),
})
```

### TerminalMetadataSnapshotEvent

Source: `packages/contracts/src/terminal.ts:132`.

```text
Object({
  type: Const("snapshot"),
  terminals: Array(TerminalSummary),
})
```

### TerminalMetadataStreamEvent

Source: `packages/contracts/src/terminal.ts:148`.

```text
Union([
  TerminalMetadataSnapshotEvent,
  TerminalMetadataUpsertEvent,
  TerminalMetadataRemoveEvent,
])
```

### TerminalMetadataUpsertEvent

Source: `packages/contracts/src/terminal.ts:137`.

```text
Object({
  type: Const("upsert"),
  terminal: TerminalSummary,
})
```

### TerminalNotRunningError

Source: `packages/contracts/src/terminal.ts:328`.

```text
Object({_tag: Const("TerminalNotRunningError"),
    threadId: String,
    terminalId: String,
  })
```

### TerminalOpenInput

Source: `packages/contracts/src/terminal.ts:36`.

```text
Object({
  ...TerminalSessionInput.fields,
  cwd: TrimmedNonEmptyStringSchema,
  worktreePath: Optional(Nullable(TrimmedNonEmptyStringSchema)),
  cols: Optional(TerminalColsSchema),
  rows: Optional(TerminalRowsSchema),
  env: Optional(TerminalEnvSchema),
  providerInstanceId: Optional(ProviderInstanceId),
})
```

### TerminalOutputEvent

Source: `packages/contracts/src/terminal.ts:167`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("output"),
  data: String,
})
```

### TerminalProviderEnvironmentError

Source: `packages/contracts/src/terminal.ts:316`.

```text
Object({_tag: Const("TerminalProviderEnvironmentError"),
    providerInstanceId: ProviderInstanceId,
    cause: Defect(),
  })
```

### TerminalProviderInstanceNotFoundError

Source: `packages/contracts/src/terminal.ts:305`.

```text
Object({_tag: Const("TerminalProviderInstanceNotFoundError"),
    providerInstanceId: ProviderInstanceId,
  })
```

### TerminalResizeError

Source: `packages/contracts/src/terminal.ts:354`.

```text
Object({_tag: Const("TerminalResizeError"),
    threadId: String,
    terminalId: String,
    terminalPid: Number,
    cols: TerminalColsSchema,
    rows: TerminalRowsSchema,
    cause: Defect(),
  })
```

### TerminalResizeInput

Source: `packages/contracts/src/terminal.ts:70`.

```text
Object({
  ...TerminalSessionInput.fields,
  cols: TerminalColsSchema,
  rows: TerminalRowsSchema,
})
```

### TerminalRestartInput

Source: `packages/contracts/src/terminal.ts:80`.

```text
Object({
  ...TerminalSessionInput.fields,
  cwd: TrimmedNonEmptyStringSchema,
  worktreePath: Optional(Nullable(TrimmedNonEmptyStringSchema)),
  cols: TerminalColsSchema,
  rows: TerminalRowsSchema,
  env: Optional(TerminalEnvSchema),
  providerInstanceId: Optional(ProviderInstanceId),
})
```

### TerminalRestartedEvent

Source: `packages/contracts/src/terminal.ts:196`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("restarted"),
  snapshot: TerminalSessionSnapshot,
})
```

### TerminalRowsSchema

Source: `packages/contracts/src/terminal.ts:12`.

```text
Int.check(isGreaterThanOrEqualTo(1)).check(
  isLessThanOrEqualTo(500),
)
```

### TerminalSessionInput

Source: `packages/contracts/src/terminal.ts:30`.

```text
Object({
  ...TerminalThreadInput.fields,
  terminalId: TerminalIdSchema,
})
```

### TerminalSessionLookupError

Source: `packages/contracts/src/terminal.ts:293`.

```text
Object({_tag: Const("TerminalSessionLookupError"),
    threadId: String,
    terminalId: String,
  })
```

### TerminalSessionSnapshot

Source: `packages/contracts/src/terminal.ts:101`.

```text
Object({
  threadId: String.check(isNonEmpty()),
  terminalId: String.check(isNonEmpty()),
  cwd: String.check(isNonEmpty()),
  worktreePath: Nullable(TrimmedNonEmptyStringSchema),
  status: TerminalSessionStatus,
  pid: Nullable(Int.check(isGreaterThan(0))),
  history: String,
  exitCode: Nullable(Int),
  exitSignal: Nullable(Int),
  label: String.check(isMaxLength(128)),
  updatedAt: String,
  sequence: Optional(Int.check(isGreaterThanOrEqualTo(0))),
})
```

### TerminalSessionStatus

Source: `packages/contracts/src/terminal.ts:98`.

```text
Enum(["starting", "running", "exited", "error"])
```

### TerminalStartedEvent

Source: `packages/contracts/src/terminal.ts:161`.

```text
Object({
  ...TerminalEventBasefields,
  type: Const("started"),
  snapshot: TerminalSessionSnapshot,
})
```

### TerminalSummary

Source: `packages/contracts/src/terminal.ts:117`.

```text
Object({
  threadId: String.check(isNonEmpty()),
  terminalId: String.check(isNonEmpty()),
  cwd: String.check(isNonEmpty()),
  worktreePath: Nullable(TrimmedNonEmptyStringSchema),
  status: TerminalSessionStatus,
  pid: Nullable(Int.check(isGreaterThan(0))),
  exitCode: Nullable(Int),
  exitSignal: Nullable(Int),
  hasRunningSubprocess: Boolean,
  label: String.check(isMaxLength(128)),
  updatedAt: String,
})
```

### TerminalThreadInput

Source: `packages/contracts/src/terminal.ts:24`.

```text
Object({
  threadId: TrimmedNonEmptyStringSchema,
})
```

### TerminalWriteError

Source: `packages/contracts/src/terminal.ts:340`.

```text
Object({_tag: Const("TerminalWriteError"),
    threadId: String,
    terminalId: String,
    terminalPid: Number,
    cause: Defect(),
  })
```

### TerminalWriteInput

Source: `packages/contracts/src/terminal.ts:64`.

```text
Object({
  ...TerminalSessionInput.fields,
  data: String.check(isNonEmpty()).check(isMaxLength(65_536)),
})
```

### TextGenerationError

Source: `packages/contracts/src/git.ts:387`.

```text
Object({_tag: Const("TextGenerationError"),
    operation: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### ThreadContextRecord

Source: `packages/contracts/src/composerContext.ts:205`.

```text
Object({
  ...recordBase,
  kind: Const("thread"),
  environmentId: EnvironmentId,
  threadId: ThreadId,
  title: ContextLabel,
})
```

### ThreadEnvMode

Source: `packages/contracts/src/environment.ts:49`.

```text
Enum(["local", "worktree"])
```

### ThreadGuestMessage

Source: `packages/contracts/src/guestShare.ts:33`.

```text
Object({
  id: GuestMessageId,
  text: String,
  status: ThreadGuestMessageStatus,
  submittedAt: IsoDateTime,
  attachments: Optional(Array(ChatAttachment)),
})
```

### ThreadGuestMessageStatus

Source: `packages/contracts/src/guestShare.ts:30`.

```text
Enum(["pending", "sent", "dismissed"])
```

### ThreadGuestShare

Source: `packages/contracts/src/guestShare.ts:43`.

```text
Object({
  id: GuestShareId,
  host: TrimmedNonEmptyString,
  createdAt: IsoDateTime,
  expiresAt: IsoDateTime,
  guestName: Nullable(TrimmedNonEmptyString),
  messages: Array(ThreadGuestMessage),
})
```

### ThreadLinkedPullRequest

Source: `packages/contracts/src/threadPullRequest.ts:19`.

```text
Object({
  projectId: ProjectId,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
  url: TrimmedNonEmptyString,
})
```

### ThreadPullRequestKey

Source: `packages/contracts/src/threadPullRequest.ts:75`.

```text
Object({
  host: TrimmedNonEmptyString,
  repository: TrimmedNonEmptyString,
  number: PositiveInt,
})
```

### ThreadPullRequestLink

Source: `packages/contracts/src/threadPullRequest.ts:106`.

```text
Object({
  ...ThreadPullRequestKey.fields,
  url: TrimmedNonEmptyString,
  repositoryWebUrl: Optional(TrimmedNonEmptyString),
  source: ThreadPullRequestLinkSource,
  linkedAt: IsoDateTime,
  snapshot: Nullable(ThreadPullRequestSnapshot),
  stack: Nullable(ThreadPullRequestStack),
  watch: Optional(ThreadPullRequestWatch),
  endedWatch: Optional(ThreadPullRequestWatch),
})
```

### ThreadPullRequestLinkSource

Source: `packages/contracts/src/threadPullRequest.ts:28`.

```text
Enum([
  "manual",
  "created",
  "agent",
  "stack",
  "stack-dismissed",
])
```

### ThreadPullRequestSnapshot

Source: `packages/contracts/src/threadPullRequest.ts:37`.

```text
Object({
  state: PullRequestState,
  title: TrimmedNonEmptyString,
  headBranch: TrimmedNonEmptyString,
  baseBranch: TrimmedNonEmptyString,
  isDraft: Boolean,
  updatedAt: Nullable(IsoDateTime),
  syncedAt: IsoDateTime,
  closedAt: Optional(Nullable(String)),
  mergedAt: Optional(Nullable(String)),
  author: Optional(Nullable(PullRequestActor)),
  additions: Optional(NonNegativeInt),
  deletions: Optional(NonNegativeInt),
  changedFiles: Optional(NonNegativeInt),
  reviewDecision: Optional(Nullable(PullRequestReviewDecision)),
  checksState: Optional(Nullable(PullRequestChecksState)),
  mergeability: Optional(PullRequestMergeability),
})
```

### ThreadPullRequestStack

Source: `packages/contracts/src/threadPullRequest.ts:65`.

```text
Object({
  kind: Const("native"),
  id: TrimmedNonEmptyString,
  number: PositiveInt,
  url: TrimmedNonEmptyString,
  base: TrimmedNonEmptyString,
  layers: Array(ThreadPullRequestStackLayer),
})
```

### ThreadPullRequestStackLayer

Source: `packages/contracts/src/threadPullRequest.ts:57`.

```text
Object({
  number: PositiveInt,
  headBranch: TrimmedNonEmptyString,
  state: PullRequestState,
})
```

### ThreadPullRequestWatch

Source: `packages/contracts/src/threadPullRequest.ts:82`.

```text
Object({
  startedAt: IsoDateTime,
  headSha: Nullable(TrimmedNonEmptyString),
  failedChecks: Array(TrimmedNonEmptyString),
  passed: Boolean,
  passedChecks: Array(TrimmedNonEmptyString),
  owedChecks: Optional(Array(TrimmedNonEmptyString)),
  remarksThrough: IsoDateTime,
  remarkIds: Array(TrimmedNonEmptyString),
  conflicting: Boolean,
  wakes: NonNegativeInt,
  checksAwaitedSince: Optional(Nullable(IsoDateTime)),
})
```

### ThreadTokenUsageSnapshot

Source: `packages/contracts/src/providerRuntime.ts:260`.

```text
Object({
  usedTokens: NonNegativeInt,
  totalProcessedTokens: Optional(NonNegativeInt),
  maxTokens: Optional(PositiveInt),
  inputTokens: Optional(NonNegativeInt),
  cachedInputTokens: Optional(NonNegativeInt),
  outputTokens: Optional(NonNegativeInt),
  reasoningOutputTokens: Optional(NonNegativeInt),
  lastUsedTokens: Optional(NonNegativeInt),
  lastInputTokens: Optional(NonNegativeInt),
  lastCachedInputTokens: Optional(NonNegativeInt),
  lastOutputTokens: Optional(NonNegativeInt),
  lastReasoningOutputTokens: Optional(NonNegativeInt),
  toolUses: Optional(NonNegativeInt),
  durationMs: Optional(NonNegativeInt),
  compactsAutomatically: Optional(Boolean),
  autoCompactThreshold: Optional(PositiveInt),
  cost: Optional(
    Object({
      amount: Number.check(isFinite()),
      currency: TrimmedNonEmptyStringcheck(isMaxLength(32)),
    }),
})
```

### TimeOfDay

Source: `packages/contracts/src/scheduledTask.ts:21`.

```text
TrimmedNonEmptyString.check(
  isPattern(/^([01]?\d|2[0-3]):([0-5]\d)$/),
)
```

### Title

Source: `packages/contracts/src/preview.ts:13`.

```text
String.check(isMaxLength(512))
```

### ToolActivityIcon

Source: `packages/contracts/src/providerRuntime.ts:406`.

```text
Union([
  TaggedObject("website", {
    pageUrl: TrimmedNonEmptyStringcheck(isMaxLength(4096)),
    faviconUrl: Optional(TrimmedNonEmptyStringcheck(isMaxLength(4096))),
    faviconUrlDark: Optional(TrimmedNonEmptyStringcheck(isMaxLength(4096))),
  }),
  TaggedObject("native-app", {
    app: ToolActivityNativeAppReference,
  }),
  TaggedObject("themed-logo", {
    logoUrl: TrimmedNonEmptyStringcheck(isMaxLength(4096)),
    logoUrlDark: Optional(TrimmedNonEmptyStringcheck(isMaxLength(4096))),
  }),
])
```

### ToolActivityNativeAppReference

Source: `packages/contracts/src/providerRuntime.ts:393`.

```text
Union([
  TaggedObject("app-id", {
    appId: TrimmedNonEmptyStringcheck(
      isMaxLength(512),
      isPattern(/^[A-Za-z0-9._-]+$/u),
    ),
  }),
  TaggedObject("display-name", {
    displayName: TrimmedNonEmptyStringcheck(isMaxLength(160)),
  }),
])
```

### ToolActivitySource

Source: `packages/contracts/src/providerRuntime.ts:422`.

```text
Object({
  key: TrimmedNonEmptyStringcheck(isMaxLength(512)),
  name: TrimmedNonEmptyStringcheck(isMaxLength(160)),
  kind: Enum(["browser", "computer", "integration"]),
  icon: Optional(ToolActivityIcon),
})
```

### ToolActivitySurface

Source: `packages/contracts/src/providerRuntime.ts:390`.

```text
Enum(["browser", "computer"])
```

### TrimmedString

Source: `packages/contracts/src/baseSchemas.ts:14`.

```text
String, trimmed on both encode and decode
```

### TurnCountRange

Source: `packages/contracts/src/checkpointDiff.ts:7`.

```text
Object({
  fromTurnCount: NonNegativeInt,
  toTurnCount: NonNegativeInt,
}).check(
  Predicate('OrchestrationTurnDiffRange'),
)
```

### TurnTokenUsage

Source: `packages/contracts/src/providerRuntime.ts:330`.

```text
Union([
  Object({
    ...TurnTokenUsageCommonFields,
    usageStatus: Const("complete"),
    inputTokens: NonNegativeInt,
    outputTokens: NonNegativeInt,
  }),
  Object({
    ...TurnTokenUsageCommonFields,
    usageStatus: Enum(["partial", "unavailable"]),
    inputTokens: Optional(NonNegativeInt),
    outputTokens: Optional(NonNegativeInt),
  }),
])
```

### TurnTokenUsageCommonFields

Source: `packages/contracts/src/providerRuntime.ts:323`.

```text
{
  usageScope: Const("main_agent"),
  cachedInputTokens: Optional(NonNegativeInt),
  cacheCreationTokens: Optional(NonNegativeInt),
  reasoningTokens: Optional(NonNegativeInt),
  hasSubagents: Boolean,
}
```

### UnknownContextRecord

Source: `packages/contracts/src/composerContext.ts:217`.

```text
Object({
  ...recordBase,
  kind: ComposerContextKind.check(isPattern(KNOWN_KIND_PATTERN)),
  payload: Unknown.check(
    Predicate('semantic validation; see constraints in sections 2, 5 and 7'),
})
```

### UploadChatAttachment

Source: `packages/contracts/src/chatAttachment.ts:227`.

```text
Union([UploadChatImageAttachment])
```

### UploadChatImageAttachment

Source: `packages/contracts/src/chatAttachment.ts:188`.

```text
Object({
  id: Optional(ChatAttachmentId),
  type: Const("image"),
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100), isPattern(/^image\
  sizeBytes: NonNegativeInt.check(isLessThanOrEqualTo(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES)),
  dataUrl: TrimmedNonEmptyString.check(
    isMaxLength(PROVIDER_SEND_TURN_MAX_IMAGE_DATA_URL_CHARS),
  ),
  source: Optional(SnapShotSource),
});
export type UploadChatImageAttachment = typeof UploadChatImageAttachment.Type;
export const ChatAttachment = Union([
  ChatImageAttachment,
  ChatFileAttachment,
  ChatUnknownAttachment,
]);
export type ChatAttachment = typeof ChatAttachment.Type;
export function getProviderAttachmentLimitError(
  attachments: ReadonlyArray<Pick<ChatAttachment, "type" | "mimeType" | "sizeBytes">>,
): string | undefined {
  if (attachments.length > PROVIDER_SEND_TURN_MAX_ATTACHMENTS) {
    return `You can attach up to ${PROVIDER_SEND_TURN_MAX_ATTACHMENTS} files per message or question response.`;
  }
  const imageBytes = attachments.reduce(
    (total, attachment) =>
      total +
      (attachment.type === "image" || isProviderSendTurnSupportedImageMimeType(attachment.mimeType)
        ? attachment.sizeBytes
        : 0),
    0,
  );
  if (imageBytes > PROVIDER_SEND_TURN_MAX_TOTAL_IMAGE_BYTES) {
    return "Images can total up to 80 MiB per message or question response. Use smaller images or send fewer at once.";
  }
}
export const UploadChatAttachment = Union([UploadChatImageAttachment]);
export type UploadChatAttachment = typeof UploadChatAttachment.Type;
export const PersistChatAttachmentsInput = Object({
  threadId: ThreadId,
  messageId: MessageId,
  attachments: Array(UploadChatAttachment),
});
export type PersistChatAttachmentsInput = typeof PersistChatAttachmentsInput.Type;
export const PersistChatAttachmentsResult = Object({
  attachments: Array(ChatAttachment),
});
export type PersistChatAttachmentsResult = typeof PersistChatAttachmentsResult.Type;
export class PersistChatAttachmentsError extends TaggedError<PersistChatAttachmentsError>()(
  "PersistChatAttachmentsError",
  {
    message: String,
    cause: Optional(Defect()),
  },
) {}
```

### Url

Source: `packages/contracts/src/preview.ts:8`.

```text
TrimmedNonEmptyString.check(isMaxLength(PREVIEW_URL_MAX_LENGTH))
```

### UsageBucket

Source: `packages/contracts/src/usage.ts:53`.

```text
Object({
  day: UsageDay,
  hourStart: Optional(TrimmedNonEmptyString),
  provider: UsageProviderKind,
  model: TrimmedNonEmptyString,
  sourcePath: Optional(TrimmedNonEmptyString),
  totals: UsageTokenTotals,
  costUsd: Number,
  cacheSavingsUsd: Number,
  categoryCostUsd: Optional(UsageCategoryCost),
  fastCostUsd: Optional(Number),
  ultrafastCostUsd: Optional(Number),
  speedPremiumUsd: Optional(Number),
  costSource: UsageCostSource,
  records: NonNegativeInt,
  unpricedRecords: NonNegativeInt,
  sessions: NonNegativeInt,
})
```

### UsageCategoryCost

Source: `packages/contracts/src/usage.ts:45`.

```text
Object({
  input: Number,
  cacheRead: Number,
  cacheWrite: Number,
  output: Number,
})
```

### UsageCostSource

Source: `packages/contracts/src/usage.ts:32`.

```text
Enum(["providerReported", "modelPriced", "unpriced"])
```

### UsageDay

Source: `packages/contracts/src/usage.ts:24`.

```text
TrimmedNonEmptyString.check(isPattern(USAGE_DAY_PATTERN))
```

### UsageLimitSourceAccount

Source: `packages/contracts/src/providerUsageLimits.ts:58`.

```text
Object({
  id: TrimmedNonEmptyString,
  driver: ProviderDriverKind,
  email: Optional(TrimmedNonEmptyString),
  plan: Optional(TrimmedNonEmptyString),
  workspaceId: Optional(TrimmedNonEmptyString),
  usageLimits: ServerProviderUsageLimits,
})
```

### UsageLimitSourceConfig

Source: `packages/contracts/src/settings.ts:807`.

```text
Object({
  kind: Const("cliproxy"),
  label: Optional(TrimmedNonEmptyString),
  url: TrimmedNonEmptyString,
  managementKey: TrimmedString,
  enabled: Boolean,
})
```

### UsageLimitSourceConsumeResetCreditInput

Source: `packages/contracts/src/providerUsageLimits.ts:81`.

```text
Object({
  sourceId: UsageLimitSourceId,
  accountId: TrimmedNonEmptyString,
  creditId: TrimmedNonEmptyString,
})
```

### UsageLimitSourceError

Source: `packages/contracts/src/providerUsageLimits.ts:95`.

```text
Object({_tag: Const("UsageLimitSourceError"), detail: String })
```

### UsageLimitSourceId

Source: `packages/contracts/src/usageLimitSourceId.ts:4`.

```text
String
```

### UsageLimitSourceSnapshot

Source: `packages/contracts/src/providerUsageLimits.ts:68`.

```text
Object({
  id: UsageLimitSourceId,
  kind: Const("cliproxy"),
  label: TrimmedNonEmptyString,
  checkedAt: IsoDateTime,
  accounts: ForwardCompatibleArray(UsageLimitSourceAccount),
  error: Optional(TrimmedNonEmptyString),
})
```

### UsageLimitSourceSnapshots

Source: `packages/contracts/src/providerUsageLimits.ts:78`.

```text
ForwardCompatibleArray(UsageLimitSourceSnapshot)
```

### UsageModelPriceOverride

Source: `packages/contracts/src/settings.ts:489`.

```text
Object({
  inputCostPerMillionTokens: UsageModelTokenPrice,
  outputCostPerMillionTokens: UsageModelTokenPrice,
  cacheReadCostPerMillionTokens: Optional(UsageModelTokenPrice),
  cacheWriteCostPerMillionTokens: Optional(UsageModelTokenPrice),
})
```

### UsageModelTokenPrice

Source: `packages/contracts/src/settings.ts:483`.

```text
Number.check(
  isFinite(),
  isGreaterThanOrEqualTo(0),
)
```

### UsagePricing

Source: `packages/contracts/src/usage.ts:103`.

```text
Object({
  status: UsagePricingStatus,
  source: TrimmedNonEmptyString,
  fetchedAt: Nullable(String),
  knownModels: NonNegativeInt,
})
```

### UsagePricingStatus

Source: `packages/contracts/src/usage.ts:100`.

```text
Enum(["fresh", "cached", "unavailable"])
```

### UsageProviderKind

Source: `packages/contracts/src/usage.ts:10`.

```text
Enum([
  "claude",
  "codex",
  "grok",
  "cursor",
  "opencode",
  "antigravity",
  "pi",
  "muse",
])
```

### UsageReadError

Source: `packages/contracts/src/usage.ts:136`.

```text
Object({_tag: Const("UsageReadError"),
  reason: Enum(["scanFailed", "invalidWindow"]),
  detail: TrimmedNonEmptyString,
  cause: Optional(Defect()),
})
```

### UsageResolution

Source: `packages/contracts/src/usage.ts:29`.

```text
Enum(["day", "hour"])
```

### UsageSource

Source: `packages/contracts/src/usage.ts:85`.

```text
Object({
  fingerprint: UsageSourceFingerprint,
  status: UsageSourceStatus,
  scannedFiles: NonNegativeInt,
  skippedFiles: NonNegativeInt,
  malformedRecords: NonNegativeInt,
  distinctSessions: NonNegativeInt,
  message: Nullable(TrimmedNonEmptyString),
  action: Optional(Const("enableCursorKeychain")),
  refreshing: Optional(Const(true)),
})
```

### UsageSourceFingerprint

Source: `packages/contracts/src/usage.ts:73`.

```text
Object({
  hostId: TrimmedNonEmptyString,
  provider: UsageProviderKind,
  resolvedHomePath: TrimmedNonEmptyString,
  volumeId: String,
})
```

### UsageSourceStatus

Source: `packages/contracts/src/usage.ts:82`.

```text
Enum(["ok", "missing", "partial", "failed"])
```

### UsageSummary

Source: `packages/contracts/src/usage.ts:123`.

```text
Object({
  contractVersion: Number,
  readAt: String,
  timeZone: TrimmedNonEmptyString,
  sinceDay: UsageDay,
  untilDay: UsageDay,
  buckets: ForwardCompatibleArray(UsageBucket),
  sources: ForwardCompatibleArray(UsageSource),
  pricing: UsagePricing,
  scanDurationMs: NonNegativeInt,
})
```

### UsageSummaryInput

Source: `packages/contracts/src/usage.ts:111`.

```text
Object({
  sinceDay: UsageDay,
  untilDay: UsageDay,
  timeZone: TrimmedNonEmptyString,
  resolution: Optional(UsageResolution),
  sinceTime: Optional(TrimmedNonEmptyString),
  untilTime: Optional(TrimmedNonEmptyString),
  awaitRefresh: Optional(Boolean),
})
```

### UsageTokenTotals

Source: `packages/contracts/src/usage.ts:36`.

```text
Object({
  uncachedInputTokens: NonNegativeInt,
  cachedInputTokens: NonNegativeInt,
  cacheCreationTokens: NonNegativeInt,
  outputTokens: NonNegativeInt,
  reasoningTokens: NonNegativeInt,
})
```

### UserInputAttachmentAnswerPayload

Source: `packages/contracts/src/providerPolicy.ts:82`.

```text
Object({
  requestId: TrimmedNonEmptyString,
  questionTextById: Optional(Record(String, String)),
  answers: ProviderUserInputAnswers,
  attachmentsByQuestionId: UserInputAttachments,
})
```

### UserInputAttachments

Source: `packages/contracts/src/providerPolicy.ts:74`.

```text
Record(
  String,
  Array(Union([ChatImageAttachment, ChatFileAttachment])),
)
```

### VcsCreateRefInput

Source: `packages/contracts/src/git.ts:181`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  refName: TrimmedNonEmptyStringSchema,
  switchRef: Optional(Boolean),
})
```

### VcsCreateRefResult

Source: `packages/contracts/src/git.ts:188`.

```text
Object({
  refName: TrimmedNonEmptyStringSchema,
})
```

### VcsCreateWorktreeInput

Source: `packages/contracts/src/git.ts:151`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  refName: TrimmedNonEmptyStringSchema,
  newRefName: Optional(TrimmedNonEmptyStringSchema),
  baseRefName: Optional(TrimmedNonEmptyStringSchema),
  path: Nullable(TrimmedNonEmptyStringSchema),
})
```

### VcsCreateWorktreeResult

Source: `packages/contracts/src/git.ts:290`.

```text
Object({
  worktree: VcsWorktree,
})
```

### VcsDiscoveryItem

Source: `packages/contracts/src/sourceControl.ts:148`.

```text
Object({
  kind: VcsDriverKind,
  implemented: Boolean,
  ...SourceControlDiscoverySharedFields,
})
```

### VcsDriverKind

Source: `packages/contracts/src/vcs.ts:4`.

```text
Enum(["git", "jj", "unknown"])
```

### VcsError

Source: `packages/contracts/src/vcs.ts:271`.

```text
Union([
  VcsProcessSpawnError,
  VcsProcessExitError,
  VcsProcessTimeoutError,
  VcsProcessStdinWriteError,
  VcsProcessOutputReadError,
  VcsProcessOutputLimitError,
  VcsProcessMissingExitCodeError,
  VcsRepositoryDetectionError,
  VcsUnsupportedOperationError,
])
```

### VcsInitInput

Source: `packages/contracts/src/git.ts:199`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  kind: Optional(VcsDriverKind),
})
```

### VcsListRefsInput

Source: `packages/contracts/src/git.ts:138`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  query: Optional(TrimmedNonEmptyStringcheck(isMaxLength(256))),
  cursor: Optional(NonNegativeInt),
  includeMatchingRemoteRefs: Optional(Boolean),
  refKind: Optional(Enum(["all", "local", "remote"])),
  refresh: Optional(Boolean),
  limit: Optional(
    PositiveInt.check(isLessThanOrEqualTo(GIT_LIST_BRANCHES_MAX_LIMIT)),
  ),
})
```

### VcsListRefsResult

Source: `packages/contracts/src/git.ts:280`.

```text
Object({
  isAtRepositoryRoot: Optional(Boolean),
  refs: Array(VcsRef),
  isRepo: Boolean,
  hasPrimaryRemote: Boolean,
  nextCursor: NonNegativeInt,
  totalCount: NonNegativeInt,
})
```

### VcsProcessBoundaryErrorFields

Source: `packages/contracts/src/vcs.ts:180`.

```text
{
  operation: String,
  command: String,
  cwd: String,
  argumentCount: Optional(NonNegativeInt),
}
```

### VcsProcessExitError

Source: `packages/contracts/src/vcs.ts:112`.

```text
Object({_tag: Const("VcsProcessExitError"),
    operation: String,
    command: String,
    cwd: String,
    argumentCount: Optional(NonNegativeInt),
    exitCode: Number,
    detail: String,
    failureKind: Optional(VcsProcessExitFailureKind),
    retryable: Optional(Boolean),
    stderrLength: Optional(NonNegativeInt),
    stderrTruncated: Optional(Boolean),
  })
```

### VcsProcessExitFailureKind

Source: `packages/contracts/src/vcs.ts:76`.

```text
Enum([
  "authentication",
  "not-found",
  "rate-limited",
  "command-failed",
])
```

### VcsProcessMissingExitCodeError

Source: `packages/contracts/src/vcs.ts:227`.

```text
Object({_tag: Const("VcsRepositoryDetectionError"),
    operation: String,
    cwd: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### VcsProcessOutputLimitError

Source: `packages/contracts/src/vcs.ts:213`.

```text
Object({_tag: Const("VcsProcessOutputLimitError"),
    ...VcsProcessBoundaryErrorFields,
    stream: Enum(["stdout", "stderr"]),
    maxBytes: NonNegativeInt,
    observedBytes: NonNegativeInt,
  })
```

### VcsProcessOutputReadError

Source: `packages/contracts/src/vcs.ts:200`.

```text
Object({_tag: Const("VcsProcessOutputReadError"),
    ...VcsProcessBoundaryErrorFields,
    stream: Enum(["stdout", "stderr", "exitCode"]),
    cause: Defect(),
  })
```

### VcsProcessSpawnError

Source: `packages/contracts/src/vcs.ts:90`.

```text
Object({_tag: Const("VcsProcessSpawnError"),
    operation: String,
    command: String,
    cwd: String,
    argumentCount: Optional(NonNegativeInt),
    cause: Defect(),
  })
```

### VcsProcessStdinWriteError

Source: `packages/contracts/src/vcs.ts:187`.

```text
Object({_tag: Const("VcsProcessStdinWriteError"),
    ...VcsProcessBoundaryErrorFields,
    stdinBytes: NonNegativeInt,
    cause: Defect(),
  })
```

### VcsProcessTimeoutError

Source: `packages/contracts/src/vcs.ts:158`.

```text
Object({_tag: Const("VcsProcessTimeoutError"),
    operation: String,
    command: String,
    cwd: String,
    argumentCount: Optional(NonNegativeInt),
    timeoutMs: Number,
  })
```

### VcsPullInput

Source: `packages/contracts/src/git.ts:121`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
})
```

### VcsPullResult

Source: `packages/contracts/src/git.ts:343`.

```text
Object({
  status: Enum(["pulled", "skipped_up_to_date"]),
  refName: TrimmedNonEmptyStringSchema,
  upstreamRef: TrimmedNonEmptyStringSchema,
})
```

### VcsRef

Source: `packages/contracts/src/git.ts:83`.

```text
Object({
  name: TrimmedNonEmptyStringSchema,
  isRemote: Optional(Boolean),
  remoteName: Optional(TrimmedNonEmptyStringSchema),
  upstreamRef: Optional(TrimmedNonEmptyStringSchema),
  current: Boolean,
  isDefault: Boolean,
  worktreePath: TrimmedNonEmptyStringSchema,
})
```

### VcsRemoveWorktreeInput

Source: `packages/contracts/src/git.ts:174`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  path: TrimmedNonEmptyStringSchema,
  force: Optional(Boolean),
})
```

### VcsRepositoryDetectionError

Source: `packages/contracts/src/vcs.ts:244`.

```text
Object({_tag: Const("VcsRepositoryDetectionError"),
    operation: String,
    cwd: String,
    detail: String,
    cause: Optional(Defect()),
  })
```

### VcsStatusChangeRequest

Source: `packages/contracts/src/git.ts:205`.

```text
Object({
  number: PositiveInt,
  title: TrimmedNonEmptyStringSchema,
  url: String,
  baseRef: TrimmedNonEmptyStringSchema,
  headRef: TrimmedNonEmptyStringSchema,
  state: VcsStatusChangeRequestState,
  isDraft: Optional(Boolean),
  updatedAt: Optional(Nullable(String)),
})
```

### VcsStatusChangeRequestState

Source: `packages/contracts/src/git.ts:52`.

```text
Enum(["open", "closed", "merged"])
```

### VcsStatusInput

Source: `packages/contracts/src/git.ts:109`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
})
```

### VcsStatusLocalResult

Source: `packages/contracts/src/git.ts:254`.

```text
Object(VcsStatusLocalShape)
```

### VcsStatusLocalShape

Source: `packages/contracts/src/git.ts:216`.

```text
{
  isRepo: Boolean,
  sourceControlProvider: Optional(SourceControlProviderInfo),
  hasPrimaryRemote: Boolean,
  isDefaultRef: Boolean,
  refName: Nullable(TrimmedNonEmptyStringSchema),
  hasWorkingTreeChanges: Boolean,
  workingTree: Object({
    rootPath: Optional(GitFilePath),
    files: Array(
      Object({
        path: GitFilePath,
        insertions: NonNegativeInt,
        deletions: NonNegativeInt,
      }),
    insertions: NonNegativeInt,
    deletions: NonNegativeInt,
  }),
  branchChanges: Optional(
    Object({
      baseRef: Nullable(TrimmedNonEmptyStringSchema),
      insertions: NonNegativeInt,
      deletions: NonNegativeInt,
    }),
}
```

### VcsStatusRemoteResult

Source: `packages/contracts/src/git.ts:257`.

```text
Object(VcsStatusRemoteShape)
```

### VcsStatusRemoteShape

Source: `packages/contracts/src/git.ts:246`.

```text
{
  hasUpstream: Boolean,
  aheadCount: NonNegativeInt,
  behindCount: NonNegativeInt,
  aheadOfDefaultCount: Optional(NonNegativeInt),
  pr: Nullable(VcsStatusChangeRequest),
}
```

### VcsStatusResult

Source: `packages/contracts/src/git.ts:260`.

```text
Object({
  ...VcsStatusLocalShape,
  ...VcsStatusRemoteShape,
})
```

### VcsStatusStreamEvent

Source: `packages/contracts/src/git.ts:266`.

```text
Union([
  TaggedObject("snapshot", {
    local: VcsStatusLocalResult,
    remote: Nullable(VcsStatusRemoteResult),
  }),
  TaggedObject("localUpdated", {
    local: VcsStatusLocalResult,
  }),
  TaggedObject("remoteUpdated", {
    remote: Nullable(VcsStatusRemoteResult),
  }),
])
```

### VcsStatusSubscriptionInput

Source: `packages/contracts/src/git.ts:114`.

```text
Object({
  ...VcsStatusInput.fields,
  includeRemote: Optional(Boolean),
})
```

### VcsSwitchRefInput

Source: `packages/contracts/src/git.ts:193`.

```text
Object({
  cwd: TrimmedNonEmptyStringSchema,
  refName: TrimmedNonEmptyStringSchema,
})
```

### VcsSwitchRefResult

Source: `packages/contracts/src/git.ts:309`.

```text
Object({
  refName: Nullable(TrimmedNonEmptyStringSchema),
})
```

### VcsUnsupportedOperationError

Source: `packages/contracts/src/vcs.ts:258`.

```text
Object({_tag: Const("VcsUnsupportedOperationError"),
    operation: String,
    kind: VcsDriverKind,
    detail: String,
  })
```

### VcsWorktree

Source: `packages/contracts/src/git.ts:95`.

```text
Object({
  path: TrimmedNonEmptyStringSchema,
  refName: TrimmedNonEmptyStringSchema,
})
```

### WORKTREE_SETUP_DETAIL_MAX_LENGTH

Source: `packages/contracts/src/worktreeSetup.ts:6`.

```text
200
```

### WORKTREE_SETUP_ERROR_MAX_LENGTH

Source: `packages/contracts/src/worktreeSetup.ts:8`.

```text
1000
```

### WORKTREE_SETUP_TAIL_LINE_MAX_LENGTH

Source: `packages/contracts/src/worktreeSetup.ts:7`.

```text
400
```

### WebhookDedupeTemplate

Source: `packages/contracts/src/scheduledTask.ts:79`.

```text
String.check(Predicate('semantic validation; see constraints in sections 2, 5 and 7'))
```

### WebhookFilterListValue

Source: `packages/contracts/src/scheduledTask.ts:101`.

```text
String.check(
  isPattern(/^[^,\s](?:[^,]*[^,\s])?$/),
)
```

### WebhookFilterPath

Source: `packages/contracts/src/scheduledTask.ts:93`.

```text
TrimmedNonEmptyString.check(
  isPattern(/^(body|headers|query)(\.|$)/),
)
```

### WebhookMaxDeliveryAgeMinutes

Source: `packages/contracts/src/scheduledTask.ts:66`.

```text
Int.check(
  isBetween({ minimum: 1, maximum: MAX_WEBHOOK_DELIVERY_AGE_MINUTES })
```

### WorktreeCleanup

Source: `packages/contracts/src/settings.ts:947`.

```text
Nullable(
  Union([
    Object({ mode: Const("off") }),
    Object({ mode: Const("custom"), rules: WorktreeCleanupRules }),
  ]),
)
```

### WorktreeCleanupRules

Source: `packages/contracts/src/settings.ts:933`.

```text
Object({
  worktreeKeepWhen: WorktreeKeepWhen,
  worktreeAfterDays: StorageRetentionDays,
  worktreeSettledAfterDays: SettledWorktreeRetentionDays,
  worktreeOnMerge: Boolean,
  worktreeOnDelete: Boolean,
  worktreeUnchanged: Boolean,
})
```

### WorktreeKeepWhen

Source: `packages/contracts/src/settings.ts:926`.

```text
Enum([
  "any-local-files",
  "uncommitted-changes",
  "tracked-changes",
])
```

### WorktreeSetupCancelInput

Source: `packages/contracts/src/worktreeSetup.ts:74`.

```text
Object({
  threadId: ThreadId,
})
```

### WorktreeSetupCancelResult

Source: `packages/contracts/src/worktreeSetup.ts:79`.

```text
Object({
  cancelled: Boolean,
})
```

### WorktreeSetupPhase

Source: `packages/contracts/src/worktreeSetup.ts:40`.

```text
Enum(["running", "done", "failed", "cancelled"])
```

### WorktreeSetupSnapshot

Source: `packages/contracts/src/worktreeSetup.ts:43`.

```text
Object({
  threadId: ThreadId,
  phase: WorktreeSetupPhase,
  startedAt: IsoDateTime,
  endedAt: Nullable(IsoDateTime),
  branch: Nullable(TrimmedNonEmptyString),
  baseRef: Nullable(TrimmedNonEmptyString),
  worktreePath: Nullable(TrimmedNonEmptyString),
  setupScript: Nullable(
    Object({
      name: TrimmedNonEmptyString,
      command: TrimmedNonEmptyString,
      terminalId: TrimmedNonEmptyString,
    }),
  stages: Array(WorktreeSetupStage),
  error: Nullable(String.check(isMaxLength(WORKTREE_SETUP_ERROR_MAX_LENGTH))),
  sequence: NonNegativeInt,
})
```

### WorktreeSetupStage

Source: `packages/contracts/src/worktreeSetup.ts:29`.

```text
Object({
  id: WorktreeSetupStageId,
  status: WorktreeSetupStageStatus,
  startedAt: Nullable(IsoDateTime),
  endedAt: Nullable(IsoDateTime),
  percent: Nullable(Int.check(isBetween({ minimum: 0, maximum: 100 })),
  detail: Nullable(String.check(isMaxLength(WORKTREE_SETUP_DETAIL_MAX_LENGTH))),
  tail: Array(String.check(isMaxLength(WORKTREE_SETUP_TAIL_LINE_MAX_LENGTH))),
})
```

### WorktreeSetupStageId

Source: `packages/contracts/src/worktreeSetup.ts:10`.

```text
Enum([
  "fetch",
  "checkout",
  "submodules",
  "setup-script",
  "agent",
])
```

### WorktreeSetupStageStatus

Source: `packages/contracts/src/worktreeSetup.ts:19`.

```text
Enum([
  "pending",
  "running",
  "done",
  "skipped",
  "warning",
  "failed",
])
```

### WorktreeSetupStreamEvent

Source: `packages/contracts/src/worktreeSetup.ts:71`.

```text
Nullable(WorktreeSetupSnapshot)
```

### WorktreeSetupSubscribeInput

Source: `packages/contracts/src/worktreeSetup.ts:66`.

```text
Object({
  threadId: ThreadId,
})
```

### WorktreeSubmodules

Source: `packages/contracts/src/environment.ts:52`.

```text
Enum(["recursive", "top-level", "none"])
```

### attachmentBinding

Source: `packages/contracts/src/composerContext.ts:87`.

```text
{
  attachmentId: ContextAttachmentId,
  name: TrimmedNonEmptyString.check(isMaxLength(255)),
  mimeType: TrimmedNonEmptyString.check(isMaxLength(100)),
  sizeBytes: NonNegativeInt,
}
```

### authScopeResponseFields

Source: `packages/contracts/src/auth.ts:93`.

```text
{
  scopes: AuthEnvironmentScopes,
  permissions: Optional(ForwardCompatibleArray(AuthEnvironmentScope)),
}
```

### deviceHostSummaryFields

Source: `packages/contracts/src/device.ts:147`.

```text
{
  id: DeviceHostId,
  label: TrimmedNonEmptyString,
  platforms: Array(DevicePlatformAvailability),
  tools: Optional(DeviceToolVersions),
  toolInspectionError: Optional(String),
  hubInstalled: Boolean,
  agentDeviceInstalled: Boolean,
}
```

### deviceServiceStateFields

Source: `packages/contracts/src/device.ts:195`.

```text
{
  supportsHostRetry: Optional(Boolean),
  supportsToolUpdate: Optional(Boolean),
  supportsToolInspection: Optional(Boolean),
  supportsEasHosts: Optional(Boolean),
  hostStatus: DeviceHostStatus,
  hostStatusDetail: Optional(String),
  hostStatuses: DeviceHostStatuses,
  devices: Array(DeviceSummary),
  sessions: Array(DeviceSession),
  bootingDevices: Optional(
    Array(Object({ ...DeviceSummary.fields, threadId: ThreadId }),
  ),
  onboardingCompleted: Boolean,
  agentAccessEnabled: Boolean,
  hubBasePath: String,
  revision: Int,
}
```

### environmentThemeFields

Source: `packages/contracts/src/server.ts:527`.

```text
{
  version: Optional(Const(1)),
  name: TrimmedNonEmptyString.check(isMaxLength(48)),
  appearance: Enum(["light", "dark"]),
  canvas: Optional(EnvironmentThemeColor),
  accent: Optional(EnvironmentThemeColor),
  colors: Optional(EnvironmentThemeColors),
  variants: Optional(
    Object({
      light: Optional(EnvironmentThemeColors),
      dark: Optional(EnvironmentThemeColors),
    }),
}
```

### isJson

Source: `packages/contracts/src/composerContext.ts:214`.

```text
is(Json)
```

### knownTags

Source: `packages/contracts/src/baseSchemas.ts:227`.

```text
set of the tag constants declared by each member of a tagged union.
```

### projectIconMembers

Source: `packages/contracts/src/project.ts:113`.

```text
[
  ProjectLucideIconWithLegacyMonogram,
  ProjectEmojiIcon,
  ProjectMonogramIcon,
]
```

### projectedTurnItemArray

Source: `packages/contracts/src/orchestrationV2.ts:1668`.

```text
Array(row); the second argument supplies the tolerant client decoder.
```

### recordBase

Source: `packages/contracts/src/composerContext.ts:81`.

```text
{
  version: Const(1),
  contextId: ComposerContextId,
  label: ContextLabel,
}
```

### slugSchema

Source: `packages/contracts/src/providerInstance.ts:10`.

```text
TrimmedNonEmptyString.check(
  isMaxLength(PROVIDER_SLUG_MAX_CHARS),
  isPattern(PROVIDER_SLUG_PATTERN),
)
```

### turnItemArray

Source: `packages/contracts/src/orchestrationV2.ts:1664`.

```text
Array(union); tolerate unknown item.type variants during decoding.
```

### uniqueHostIds

Source: `packages/contracts/src/device.ts:73`.

```text
predicate: every host.id in the array is unique.
```

### viewportAreaFilter

Source: `packages/contracts/src/preview.ts:29`.

```text
predicate: width * height <= PREVIEW_VIEWPORT_MAX_AREA.
```

### withRecordIdTable

Source: `packages/contracts/src/orchestrationV2IdTable.ts:95`.

```text
schema with optional record ID table and substitutions from section 5.5. No helper function is serialized.
```


### AuthSessionId

Source: `packages/contracts/src/baseSchemas.ts:274`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### CONTEXT_ID_PATTERN

Source: `packages/contracts/src/composerContext.ts:31`.

```text
/^[a-z0-9_-]+$/i
```

### CONTEXT_KIND_PATTERN

Source: `packages/contracts/src/composerContext.ts:32`.

```text
/^[a-z][a-z0-9-]{0,39}$/
```

### CheckpointId

Source: `packages/contracts/src/baseSchemas.ts:330`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### CheckpointRef

Source: `packages/contracts/src/baseSchemas.ts:328`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### CheckpointScopeId

Source: `packages/contracts/src/baseSchemas.ts:332`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### CommandId

Source: `packages/contracts/src/baseSchemas.ts:256`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ContextHandoffId

Source: `packages/contracts/src/baseSchemas.ts:334`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ContextTransferId

Source: `packages/contracts/src/baseSchemas.ts:336`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ENVIRONMENT_VARIABLE_NAME_PATTERN

Source: `packages/contracts/src/providerInstance.ts:8`.

```text
/^[a-zA-Z_][a-zA-Z0-9_]*$/
```

### EnvironmentId

Source: `packages/contracts/src/baseSchemas.ts:254`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### EventId

Source: `packages/contracts/src/baseSchemas.ts:258`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### GitPullRequestReference

Source: `packages/contracts/src/git.ts:53`.

```text
TrimmedNonEmptyStringSchema
```

### GuestMessageId

Source: `packages/contracts/src/baseSchemas.ts:272`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### GuestShareId

Source: `packages/contracts/src/baseSchemas.ts:270`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### KNOWN_KIND_PATTERN

Source: `packages/contracts/src/composerContext.ts:33`.

```text
string pattern excluding the known COMPOSER_CONTEXT_KINDS; used only for forward-compatible context records.
```

### MessageId

Source: `packages/contracts/src/baseSchemas.ts:260`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### NAME_BREAKING_CHARACTER

Source: `packages/contracts/src/guestShare.ts:94`.

```text
/[\p{Cc}\p{Zl}\p{Zp}\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/u
```

### NodeId

Source: `packages/contracts/src/baseSchemas.ts:268`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### OrchestrationGetFullThreadDiffResult

Source: `packages/contracts/src/checkpointDiff.ts:48`.

```text
ThreadTurnDiff
```

### OrchestrationGetTurnDiffResult

Source: `packages/contracts/src/checkpointDiff.ts:38`.

```text
ThreadTurnDiff
```

### OrchestrationV2ProjectedTurnItemJson

Source: `packages/contracts/src/orchestrationV2.ts:2491`.

```text
MergeFields(OrchestrationV2ProjectedTurnItem, {item: OrchestrationV2TurnItem}); timestamps in JSON use strings.
```

### PROVIDER_SLUG_PATTERN

Source: `packages/contracts/src/providerInstance.ts:6`.

```text
/^[a-zA-Z][a-zA-Z0-9_-]*$/
```

### PlanId

Source: `packages/contracts/src/baseSchemas.ts:340`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ProjectId

Source: `packages/contracts/src/baseSchemas.ts:252`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ProviderSessionId

Source: `packages/contracts/src/baseSchemas.ts:305`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ProviderThreadId

Source: `packages/contracts/src/baseSchemas.ts:307`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ProviderTurnId

Source: `packages/contracts/src/baseSchemas.ts:309`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### RawEventId

Source: `packages/contracts/src/baseSchemas.ts:338`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### RelayCloudEnvironmentHealthProof

Source: `packages/contracts/src/relay.ts:898`.

```text
TrimmedNonEmptyString
```

### RelayCloudMintCredentialProof

Source: `packages/contracts/src/relay.ts:871`.

```text
TrimmedNonEmptyString
```

### RunAttemptId

Source: `packages/contracts/src/baseSchemas.ts:266`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### RunId

Source: `packages/contracts/src/baseSchemas.ts:264`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### RuntimeRequestId

Source: `packages/contracts/src/baseSchemas.ts:317`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ScheduledTaskId

Source: `packages/contracts/src/baseSchemas.ts:321`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### SecretRef

Source: `packages/contracts/src/baseSchemas.ts:323`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### ShortString

Source: `packages/contracts/src/composerContext.ts:67`.

```text
BoundedString(2_048)
```

### ThreadId

Source: `packages/contracts/src/baseSchemas.ts:250`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### TrimmedNonEmptyString

Source: `packages/contracts/src/baseSchemas.ts:29`.

```text
TrimmedString.check(isNonBlank)
```

### TrimmedNonEmptyStringSchema

Source: `packages/contracts/src/git.ts:14`.

```text
TrimmedNonEmptyString
```

### TurnItemId

Source: `packages/contracts/src/baseSchemas.ts:315`.

```text
TrimmedNonEmptyString (branded identity; plain string in JSON)
```

### USAGE_DAY_PATTERN

Source: `packages/contracts/src/usage.ts:22`.

```text
/^\d{4}-\d{2}-\d{2}$/
```

### pendingGuestMessageCount

Source: `packages/contracts/src/guestShare.ts:55`.

```text
number of share.messages whose status is "pending"; zero for an absent/null share.
```

### COMPOSER_CONTEXT_KINDS

Source: `packages/contracts/src/composerContext.ts:17`.

```text
[
  "image",
  "file",
  "terminal",
  "element",
  "preview-annotation",
  "review-comment",
  "mention",
  "skill",
  "thread",
]
```

### ThreadTurnDiff

Source: `packages/contracts/src/checkpointDiff.ts:21`.

```text
TurnCountRange.mapFields(
  Overlay({
    threadId: ThreadId,
    diff: String,
  }),
  { unsafePreserveChecks: true },
)
```

### isNonBlank

Source: `packages/contracts/src/baseSchemas.ts:24`.

```text
Predicate('a non-blank string')
```

### trim

Source: `packages/contracts/src/baseSchemas.ts:9`.

```text
trim surrounding whitespace; preserve content otherwise.
```

## Appendix D. SQLite migrations and DDL

This is an ordered **migration-source DDL inventory**, not a ready-to-run migration script. Data-bearing migrations also execute validated JSON transformations, backfills, conditional operations and table cleanup. Skipping those operations can leave a structurally valid but semantically incompatible database. Dynamic `${...}` operands remain marked explicitly. Ledger management is Effect SQL Migrator's responsibility (`apps/server/src/persistence/Migrations.ts:1`); preserve the existing ledger when importing, rather than resetting migration numbers.

Migration 55 executes Base → Foundation → ThreadLaunchWorkflows → ProviderSessionBindings → Subagents → EffectCancellation → ScheduledTasks → ApplicationEventSource → LegacyV1ImportState → ApplicationEventSequenceIndexes → RecoveryIndexes → ShellIndexes. Its original v2 event/receipt tables are intermediate sources; ApplicationEventSource folds them into the shared application log. DDL below follows that nesting order. Remaining numbered migrations run in ascending number. Each block shows exact source line for the SQL and preserves original column/index spelling.

### Migration 001_OrchestrationEvents

Source: `apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:1`.

`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:7`

```sql
CREATE TABLE IF NOT EXISTS orchestration_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  aggregate_kind TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  command_id TEXT,
  causation_event_id TEXT,
  correlation_id TEXT,
  actor_kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:25`

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_orch_events_stream_version
ON orchestration_events(aggregate_kind, stream_id, stream_version);
```

`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:30`

```sql
CREATE INDEX IF NOT EXISTS idx_orch_events_stream_sequence
ON orchestration_events(aggregate_kind, stream_id, sequence);
```

`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:35`

```sql
CREATE INDEX IF NOT EXISTS idx_orch_events_command_id
ON orchestration_events(command_id);
```

`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:40`

```sql
CREATE INDEX IF NOT EXISTS idx_orch_events_correlation_id
ON orchestration_events(correlation_id);
```

### Migration 002_OrchestrationCommandReceipts

Source: `apps/server/src/persistence/Migrations/002_OrchestrationCommandReceipts.ts:1`.

`apps/server/src/persistence/Migrations/002_OrchestrationCommandReceipts.ts:7`

```sql
CREATE TABLE IF NOT EXISTS orchestration_command_receipts (
  command_id TEXT PRIMARY KEY,
  aggregate_kind TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  accepted_at TEXT NOT NULL,
  result_sequence INTEGER NOT NULL,
  status TEXT NOT NULL,
  error TEXT
);
```

`apps/server/src/persistence/Migrations/002_OrchestrationCommandReceipts.ts:19`

```sql
CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_aggregate
ON orchestration_command_receipts(aggregate_kind, aggregate_id);
```

`apps/server/src/persistence/Migrations/002_OrchestrationCommandReceipts.ts:24`

```sql
CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_sequence
ON orchestration_command_receipts(result_sequence);
```

### Migration 003_CheckpointDiffBlobs

Source: `apps/server/src/persistence/Migrations/003_CheckpointDiffBlobs.ts:1`.

`apps/server/src/persistence/Migrations/003_CheckpointDiffBlobs.ts:7`

```sql
CREATE TABLE IF NOT EXISTS checkpoint_diff_blobs (
  thread_id TEXT NOT NULL,
  from_turn_count INTEGER NOT NULL,
  to_turn_count INTEGER NOT NULL,
  diff TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (thread_id, from_turn_count, to_turn_count)
);
```

`apps/server/src/persistence/Migrations/003_CheckpointDiffBlobs.ts:18`

```sql
CREATE INDEX IF NOT EXISTS idx_checkpoint_diff_blobs_thread_to_turn
ON checkpoint_diff_blobs(thread_id, to_turn_count);
```

### Migration 004_ProviderSessionRuntime

Source: `apps/server/src/persistence/Migrations/004_ProviderSessionRuntime.ts:1`.

`apps/server/src/persistence/Migrations/004_ProviderSessionRuntime.ts:7`

```sql
CREATE TABLE IF NOT EXISTS provider_session_runtime (
  thread_id TEXT PRIMARY KEY,
  provider_name TEXT NOT NULL,
  adapter_key TEXT NOT NULL,
  runtime_mode TEXT NOT NULL DEFAULT 'full-access',
  status TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  resume_cursor_json TEXT,
  runtime_payload_json TEXT
);
```

`apps/server/src/persistence/Migrations/004_ProviderSessionRuntime.ts:20`

```sql
CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_status
ON provider_session_runtime(status);
```

`apps/server/src/persistence/Migrations/004_ProviderSessionRuntime.ts:25`

```sql
CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_provider
ON provider_session_runtime(provider_name);
```

### Migration 005_Projections

Source: `apps/server/src/persistence/Migrations/005_Projections.ts:1`.

`apps/server/src/persistence/Migrations/005_Projections.ts:7`

```sql
CREATE TABLE IF NOT EXISTS projection_projects (
  project_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  workspace_root TEXT NOT NULL,
  default_model TEXT,
  scripts_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:20`

```sql
CREATE TABLE IF NOT EXISTS projection_threads (
  thread_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  model TEXT NOT NULL,
  branch TEXT,
  worktree_path TEXT,
  latest_turn_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:35`

```sql
CREATE TABLE IF NOT EXISTS projection_thread_messages (
  message_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  role TEXT NOT NULL,
  text TEXT NOT NULL,
  is_streaming INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:48`

```sql
CREATE TABLE IF NOT EXISTS projection_thread_activities (
  activity_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  tone TEXT NOT NULL,
  kind TEXT NOT NULL,
  summary TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:61`

```sql
CREATE TABLE IF NOT EXISTS projection_thread_sessions (
  thread_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  provider_name TEXT,
  provider_session_id TEXT,
  provider_thread_id TEXT,
  active_turn_id TEXT,
  last_error TEXT,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:74`

```sql
CREATE TABLE IF NOT EXISTS projection_turns (
  row_id INTEGER PRIMARY KEY AUTOINCREMENT,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  pending_message_id TEXT,
  assistant_message_id TEXT,
  state TEXT NOT NULL,
  requested_at TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  checkpoint_turn_count INTEGER,
  checkpoint_ref TEXT,
  checkpoint_status TEXT,
  checkpoint_files_json TEXT NOT NULL,
  UNIQUE (thread_id, turn_id),
  UNIQUE (thread_id, checkpoint_turn_count)
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:94`

```sql
CREATE TABLE IF NOT EXISTS projection_pending_approvals (
  request_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  status TEXT NOT NULL,
  decision TEXT,
  created_at TEXT NOT NULL,
  resolved_at TEXT
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:106`

```sql
CREATE TABLE IF NOT EXISTS projection_state (
  projector TEXT PRIMARY KEY,
  last_applied_sequence INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:114`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_projects_updated_at
ON projection_projects(updated_at);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:119`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_id
ON projection_threads(project_id);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:124`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_messages_thread_created
ON projection_thread_messages(thread_id, created_at);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:129`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_activities_thread_created
ON projection_thread_activities(thread_id, created_at);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:134`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_sessions_provider_session
ON projection_thread_sessions(provider_session_id);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:139`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_turns_thread_requested
ON projection_turns(thread_id, requested_at);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:144`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_turns_thread_checkpoint_completed
ON projection_turns(thread_id, checkpoint_turn_count, completed_at);
```

`apps/server/src/persistence/Migrations/005_Projections.ts:149`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_pending_approvals_thread_status
ON projection_pending_approvals(thread_id, status);
```

### Migration 006_ProjectionThreadSessionRuntimeModeColumns

Source: `apps/server/src/persistence/Migrations/006_ProjectionThreadSessionRuntimeModeColumns.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/006_ProjectionThreadSessionRuntimeModeColumns.ts:9`

```sql
ALTER TABLE projection_thread_sessions
ADD COLUMN runtime_mode TEXT NOT NULL DEFAULT 'full-access';
```

### Migration 007_ProjectionThreadMessageAttachments

Source: `apps/server/src/persistence/Migrations/007_ProjectionThreadMessageAttachments.ts:1`.

`apps/server/src/persistence/Migrations/007_ProjectionThreadMessageAttachments.ts:7`

```sql
ALTER TABLE projection_thread_messages
ADD COLUMN attachments_json TEXT;
```

### Migration 008_ProjectionThreadActivitySequence

Source: `apps/server/src/persistence/Migrations/008_ProjectionThreadActivitySequence.ts:1`.

`apps/server/src/persistence/Migrations/008_ProjectionThreadActivitySequence.ts:7`

```sql
ALTER TABLE projection_thread_activities
ADD COLUMN sequence INTEGER;
```

`apps/server/src/persistence/Migrations/008_ProjectionThreadActivitySequence.ts:12`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_activities_thread_sequence
ON projection_thread_activities(thread_id, sequence);
```

### Migration 009_ProviderSessionRuntimeMode

Source: `apps/server/src/persistence/Migrations/009_ProviderSessionRuntimeMode.ts:1`.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 010_ProjectionThreadsRuntimeMode

Source: `apps/server/src/persistence/Migrations/010_ProjectionThreadsRuntimeMode.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/010_ProjectionThreadsRuntimeMode.ts:7`

```sql
ALTER TABLE projection_threads
ADD COLUMN runtime_mode TEXT NOT NULL DEFAULT 'full-access';
```

### Migration 011_OrchestrationThreadCreatedRuntimeMode

Source: `apps/server/src/persistence/Migrations/011_OrchestrationThreadCreatedRuntimeMode.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 012_ProjectionThreadsInteractionMode

Source: `apps/server/src/persistence/Migrations/012_ProjectionThreadsInteractionMode.ts:1`.

`apps/server/src/persistence/Migrations/012_ProjectionThreadsInteractionMode.ts:7`

```sql
ALTER TABLE projection_threads
ADD COLUMN interaction_mode TEXT NOT NULL DEFAULT 'default';
```

### Migration 013_ProjectionThreadProposedPlans

Source: `apps/server/src/persistence/Migrations/013_ProjectionThreadProposedPlans.ts:1`.

`apps/server/src/persistence/Migrations/013_ProjectionThreadProposedPlans.ts:7`

```sql
CREATE TABLE IF NOT EXISTS projection_thread_proposed_plans (
  plan_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  plan_markdown TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/013_ProjectionThreadProposedPlans.ts:18`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_proposed_plans_thread_created
ON projection_thread_proposed_plans(thread_id, created_at);
```

### Migration 014_ProjectionThreadProposedPlanImplementation

Source: `apps/server/src/persistence/Migrations/014_ProjectionThreadProposedPlanImplementation.ts:1`.

`apps/server/src/persistence/Migrations/014_ProjectionThreadProposedPlanImplementation.ts:7`

```sql
ALTER TABLE projection_thread_proposed_plans
ADD COLUMN implemented_at TEXT;
```

`apps/server/src/persistence/Migrations/014_ProjectionThreadProposedPlanImplementation.ts:12`

```sql
ALTER TABLE projection_thread_proposed_plans
ADD COLUMN implementation_thread_id TEXT;
```

### Migration 015_ProjectionTurnsSourceProposedPlan

Source: `apps/server/src/persistence/Migrations/015_ProjectionTurnsSourceProposedPlan.ts:1`.

`apps/server/src/persistence/Migrations/015_ProjectionTurnsSourceProposedPlan.ts:7`

```sql
ALTER TABLE projection_turns
ADD COLUMN source_proposed_plan_thread_id TEXT;
```

`apps/server/src/persistence/Migrations/015_ProjectionTurnsSourceProposedPlan.ts:12`

```sql
ALTER TABLE projection_turns
ADD COLUMN source_proposed_plan_id TEXT;
```

### Migration 016_CanonicalizeModelSelections

Source: `apps/server/src/persistence/Migrations/016_CanonicalizeModelSelections.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/016_CanonicalizeModelSelections.ts:7`

```sql
ALTER TABLE projection_projects
ADD COLUMN default_model_selection_json TEXT;
```

`apps/server/src/persistence/Migrations/016_CanonicalizeModelSelections.ts:29`

```sql
ALTER TABLE projection_threads
ADD COLUMN model_selection_json TEXT;
```

`apps/server/src/persistence/Migrations/016_CanonicalizeModelSelections.ts:56`

```sql
ALTER TABLE projection_projects
DROP COLUMN default_model;
```

`apps/server/src/persistence/Migrations/016_CanonicalizeModelSelections.ts:61`

```sql
ALTER TABLE projection_threads
DROP COLUMN model;
```

### Migration 017_ProjectionThreadsArchivedAt

Source: `apps/server/src/persistence/Migrations/017_ProjectionThreadsArchivedAt.ts:1`.

`apps/server/src/persistence/Migrations/017_ProjectionThreadsArchivedAt.ts:14`

```sql
ALTER TABLE projection_threads
ADD COLUMN archived_at TEXT;
```

### Migration 018_ProjectionThreadsArchivedAtIndex

Source: `apps/server/src/persistence/Migrations/018_ProjectionThreadsArchivedAtIndex.ts:1`.

`apps/server/src/persistence/Migrations/018_ProjectionThreadsArchivedAtIndex.ts:7`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_archived_at
ON projection_threads(project_id, archived_at);
```

### Migration 019_ProjectionSnapshotLookupIndexes

Source: `apps/server/src/persistence/Migrations/019_ProjectionSnapshotLookupIndexes.ts:1`.

`apps/server/src/persistence/Migrations/019_ProjectionSnapshotLookupIndexes.ts:7`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_projects_workspace_root_deleted_at
ON projection_projects(workspace_root, deleted_at);
```

`apps/server/src/persistence/Migrations/019_ProjectionSnapshotLookupIndexes.ts:12`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_deleted_created
ON projection_threads(project_id, deleted_at, created_at);
```

### Migration 020_AuthAccessManagement

Source: `apps/server/src/persistence/Migrations/020_AuthAccessManagement.ts:1`.

`apps/server/src/persistence/Migrations/020_AuthAccessManagement.ts:7`

```sql
CREATE TABLE IF NOT EXISTS auth_pairing_links (
  id TEXT PRIMARY KEY,
  credential TEXT NOT NULL UNIQUE,
  method TEXT NOT NULL,
  role TEXT NOT NULL,
  subject TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT,
  revoked_at TEXT
);
```

`apps/server/src/persistence/Migrations/020_AuthAccessManagement.ts:21`

```sql
CREATE INDEX IF NOT EXISTS idx_auth_pairing_links_active
ON auth_pairing_links(revoked_at, consumed_at, expires_at);
```

`apps/server/src/persistence/Migrations/020_AuthAccessManagement.ts:26`

```sql
CREATE TABLE IF NOT EXISTS auth_sessions (
  session_id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  role TEXT NOT NULL,
  method TEXT NOT NULL,
  issued_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  revoked_at TEXT
);
```

`apps/server/src/persistence/Migrations/020_AuthAccessManagement.ts:38`

```sql
CREATE INDEX IF NOT EXISTS idx_auth_sessions_active
ON auth_sessions(revoked_at, expires_at, issued_at);
```

### Migration 021_AuthSessionClientMetadata

Source: `apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:1`.

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:11`

```sql
ALTER TABLE auth_pairing_links
ADD COLUMN label TEXT;
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:22`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_label TEXT;
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:29`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_ip_address TEXT;
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:36`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_user_agent TEXT;
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:43`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_device_type TEXT NOT NULL DEFAULT 'unknown';
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:50`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_os TEXT;
```

`apps/server/src/persistence/Migrations/021_AuthSessionClientMetadata.ts:57`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_browser TEXT;
```

### Migration 022_AuthSessionLastConnectedAt

Source: `apps/server/src/persistence/Migrations/022_AuthSessionLastConnectedAt.ts:1`.

`apps/server/src/persistence/Migrations/022_AuthSessionLastConnectedAt.ts:12`

```sql
ALTER TABLE auth_sessions
ADD COLUMN last_connected_at TEXT;
```

### Migration 023_ProjectionThreadShellSummary

Source: `apps/server/src/persistence/Migrations/023_ProjectionThreadShellSummary.ts:1`.

`apps/server/src/persistence/Migrations/023_ProjectionThreadShellSummary.ts:7`

```sql
ALTER TABLE projection_threads
ADD COLUMN latest_user_message_at TEXT;
```

`apps/server/src/persistence/Migrations/023_ProjectionThreadShellSummary.ts:12`

```sql
ALTER TABLE projection_threads
ADD COLUMN pending_approval_count INTEGER NOT NULL DEFAULT 0;
```

`apps/server/src/persistence/Migrations/023_ProjectionThreadShellSummary.ts:17`

```sql
ALTER TABLE projection_threads
ADD COLUMN pending_user_input_count INTEGER NOT NULL DEFAULT 0;
```

`apps/server/src/persistence/Migrations/023_ProjectionThreadShellSummary.ts:22`

```sql
ALTER TABLE projection_threads
ADD COLUMN has_actionable_proposed_plan INTEGER NOT NULL DEFAULT 0;
```

### Migration 024_BackfillProjectionThreadShellSummary

Source: `apps/server/src/persistence/Migrations/024_BackfillProjectionThreadShellSummary.ts:1`.

Also performs or prepares data operations: INSERT, UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 025_CleanupInvalidProjectionPendingApprovals

Source: `apps/server/src/persistence/Migrations/025_CleanupInvalidProjectionPendingApprovals.ts:1`.

Also performs or prepares data operations: DELETE, UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 026_CanonicalizeModelSelectionOptions

Source: `apps/server/src/persistence/Migrations/026_CanonicalizeModelSelectionOptions.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 027_ProviderSessionRuntimeInstanceId

Source: `apps/server/src/persistence/Migrations/027_ProviderSessionRuntimeInstanceId.ts:1`.

`apps/server/src/persistence/Migrations/027_ProviderSessionRuntimeInstanceId.ts:12`

```sql
ALTER TABLE provider_session_runtime
ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/027_ProviderSessionRuntimeInstanceId.ts:18`

```sql
CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_instance
ON provider_session_runtime(provider_instance_id);
```

### Migration 028_ProjectionThreadSessionInstanceId

Source: `apps/server/src/persistence/Migrations/028_ProjectionThreadSessionInstanceId.ts:1`.

`apps/server/src/persistence/Migrations/028_ProjectionThreadSessionInstanceId.ts:11`

```sql
ALTER TABLE projection_thread_sessions
ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/028_ProjectionThreadSessionInstanceId.ts:17`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_sessions_instance
ON projection_thread_sessions(provider_instance_id);
```

### Migration 029_ProjectionThreadDetailOrderingIndexes

Source: `apps/server/src/persistence/Migrations/029_ProjectionThreadDetailOrderingIndexes.ts:1`.

`apps/server/src/persistence/Migrations/029_ProjectionThreadDetailOrderingIndexes.ts:7`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_activities_thread_sequence_created_id
ON projection_thread_activities(thread_id, sequence, created_at, activity_id);
```

`apps/server/src/persistence/Migrations/029_ProjectionThreadDetailOrderingIndexes.ts:12`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_messages_thread_created_id
ON projection_thread_messages(thread_id, created_at, message_id);
```

### Migration 030_ProjectionThreadShellArchiveIndexes

Source: `apps/server/src/persistence/Migrations/030_ProjectionThreadShellArchiveIndexes.ts:1`.

`apps/server/src/persistence/Migrations/030_ProjectionThreadShellArchiveIndexes.ts:7`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_active
ON projection_threads(deleted_at, archived_at, project_id, created_at, thread_id);
```

`apps/server/src/persistence/Migrations/030_ProjectionThreadShellArchiveIndexes.ts:12`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_archived
ON projection_threads(deleted_at, archived_at, project_id, thread_id);
```

### Migration 031_AuthAuthorizationScopes

Source: `apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:1`.

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:8`

```sql
DROP TABLE IF EXISTS auth_pairing_links;
```

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:9`

```sql
DROP TABLE IF EXISTS auth_sessions;
```

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:11`

```sql
CREATE TABLE auth_pairing_links (
  id TEXT PRIMARY KEY,
  credential TEXT NOT NULL UNIQUE,
  method TEXT NOT NULL,
  scopes TEXT NOT NULL,
  subject TEXT NOT NULL,
  label TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT,
  revoked_at TEXT
);
```

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:26`

```sql
CREATE INDEX idx_auth_pairing_links_active
ON auth_pairing_links(revoked_at, consumed_at, expires_at);
```

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:31`

```sql
CREATE TABLE auth_sessions (
  session_id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  scopes TEXT NOT NULL,
  method TEXT NOT NULL,
  client_label TEXT,
  client_ip_address TEXT,
  client_user_agent TEXT,
  client_device_type TEXT NOT NULL DEFAULT 'unknown',
  client_os TEXT,
  client_browser TEXT,
  issued_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  last_connected_at TEXT,
  revoked_at TEXT
);
```

`apps/server/src/persistence/Migrations/031_AuthAuthorizationScopes.ts:50`

```sql
CREATE INDEX idx_auth_sessions_active
ON auth_sessions(revoked_at, expires_at, issued_at);
```

### Migration 032_AuthPairingProofKeyThumbprint

Source: `apps/server/src/persistence/Migrations/032_AuthPairingProofKeyThumbprint.ts:1`.

`apps/server/src/persistence/Migrations/032_AuthPairingProofKeyThumbprint.ts:11`

```sql
ALTER TABLE auth_pairing_links
ADD COLUMN proof_key_thumbprint TEXT;
```

### Migration 033_ProjectionThreadsSettled

Source: `apps/server/src/persistence/Migrations/033_ProjectionThreadsSettled.ts:1`.

`apps/server/src/persistence/Migrations/033_ProjectionThreadsSettled.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN settled_override TEXT;
```

`apps/server/src/persistence/Migrations/033_ProjectionThreadsSettled.ts:18`

```sql
ALTER TABLE projection_threads
ADD COLUMN settled_at TEXT;
```

### Migration 034_ProjectionThreadsSnoozed

Source: `apps/server/src/persistence/Migrations/034_ProjectionThreadsSnoozed.ts:1`.

`apps/server/src/persistence/Migrations/034_ProjectionThreadsSnoozed.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN snoozed_until TEXT;
```

`apps/server/src/persistence/Migrations/034_ProjectionThreadsSnoozed.ts:18`

```sql
ALTER TABLE projection_threads
ADD COLUMN snoozed_at TEXT;
```

### Migration 035_ProjectionThreadTitleRegeneration

Source: `apps/server/src/persistence/Migrations/035_ProjectionThreadTitleRegeneration.ts:1`.

`apps/server/src/persistence/Migrations/035_ProjectionThreadTitleRegeneration.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN title_regeneration_request_id TEXT;
```

`apps/server/src/persistence/Migrations/035_ProjectionThreadTitleRegeneration.ts:18`

```sql
ALTER TABLE projection_threads
ADD COLUMN title_regeneration_started_at TEXT;
```

### Migration 036_ProjectionThreadsPinned

Source: `apps/server/src/persistence/Migrations/036_ProjectionThreadsPinned.ts:1`.

`apps/server/src/persistence/Migrations/036_ProjectionThreadsPinned.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN pinned_at TEXT;
```

### Migration 037_ProjectionTurnsKeysetIndex

Source: `apps/server/src/persistence/Migrations/037_ProjectionTurnsKeysetIndex.ts:1`.

`apps/server/src/persistence/Migrations/037_ProjectionTurnsKeysetIndex.ts:7`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_turns_thread_keyset
ON projection_turns(thread_id, requested_at, turn_id);
```

### Migration 038_ProjectionThreadsPinOrderKey

Source: `apps/server/src/persistence/Migrations/038_ProjectionThreadsPinOrderKey.ts:1`.

`apps/server/src/persistence/Migrations/038_ProjectionThreadsPinOrderKey.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN pin_order_key TEXT;
```

### Migration 039_ProjectionProjectsDefaultThreadEnvMode

Source: `apps/server/src/persistence/Migrations/039_ProjectionProjectsDefaultThreadEnvMode.ts:1`.

`apps/server/src/persistence/Migrations/039_ProjectionProjectsDefaultThreadEnvMode.ts:11`

```sql
ALTER TABLE projection_projects
ADD COLUMN default_thread_env_mode TEXT;
```

### Migration 040_ProjectionProjectFaviconPath

Source: `apps/server/src/persistence/Migrations/040_ProjectionProjectFaviconPath.ts:1`.

`apps/server/src/persistence/Migrations/040_ProjectionProjectFaviconPath.ts:11`

```sql
ALTER TABLE projection_projects
ADD COLUMN favicon_path TEXT;
```

### Migration 041_AuthSessionClientConnection

Source: `apps/server/src/persistence/Migrations/041_AuthSessionClientConnection.ts:1`.

`apps/server/src/persistence/Migrations/041_AuthSessionClientConnection.ts:11`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_surface TEXT;
```

`apps/server/src/persistence/Migrations/041_AuthSessionClientConnection.ts:18`

```sql
ALTER TABLE auth_sessions
ADD COLUMN client_app_version TEXT;
```

### Migration 042_ProjectionThreadLinkedPullRequest

Source: `apps/server/src/persistence/Migrations/042_ProjectionThreadLinkedPullRequest.ts:1`.

`apps/server/src/persistence/Migrations/042_ProjectionThreadLinkedPullRequest.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN linked_pull_request_json TEXT;
```

### Migration 043_ProjectionThreadsUnsettledAt

Source: `apps/server/src/persistence/Migrations/043_ProjectionThreadsUnsettledAt.ts:1`.

`apps/server/src/persistence/Migrations/043_ProjectionThreadsUnsettledAt.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN unsettled_at TEXT;
```

### Migration 044_ClearAutomaticProjectModelDefaults

Source: `apps/server/src/persistence/Migrations/044_ClearAutomaticProjectModelDefaults.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 045_ProjectionProjectsAutoPull

Source: `apps/server/src/persistence/Migrations/045_ProjectionProjectsAutoPull.ts:1`.

`apps/server/src/persistence/Migrations/045_ProjectionProjectsAutoPull.ts:11`

```sql
ALTER TABLE projection_projects
ADD COLUMN auto_pull INTEGER NOT NULL DEFAULT 0;
```

### Migration 046_RepairAutomaticSettlementTimestamps

Source: `apps/server/src/persistence/Migrations/046_RepairAutomaticSettlementTimestamps.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 047_ProjectionProjectIcon

Source: `apps/server/src/persistence/Migrations/047_ProjectionProjectIcon.ts:1`.

`apps/server/src/persistence/Migrations/047_ProjectionProjectIcon.ts:11`

```sql
ALTER TABLE projection_projects
ADD COLUMN project_icon_json TEXT;
```

### Migration 048_ProjectionThreadBranchPullRequest

Source: `apps/server/src/persistence/Migrations/048_ProjectionThreadBranchPullRequest.ts:1`.

`apps/server/src/persistence/Migrations/048_ProjectionThreadBranchPullRequest.ts:11`

```sql
ALTER TABLE projection_threads
ADD COLUMN branch_pull_request_json TEXT;
```

### Migration 049_ProjectionThreadsActiveOrderKey

Source: `apps/server/src/persistence/Migrations/049_ProjectionThreadsActiveOrderKey.ts:1`.

`apps/server/src/persistence/Migrations/049_ProjectionThreadsActiveOrderKey.ts:10`

```sql
ALTER TABLE projection_threads
ADD COLUMN active_order_key TEXT;
```

### Migration 050_ProjectionThreadPullRequests

Source: `apps/server/src/persistence/Migrations/050_ProjectionThreadPullRequests.ts:1`.

Also performs or prepares data operations: INSERT. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/050_ProjectionThreadPullRequests.ts:34`

```sql
CREATE TABLE IF NOT EXISTS projection_thread_pull_requests (
  thread_id TEXT NOT NULL,
  host TEXT NOT NULL,
  repository TEXT NOT NULL,
  number INTEGER NOT NULL,
  url TEXT NOT NULL,
  source TEXT NOT NULL,
  linked_at TEXT NOT NULL,
  snapshot_json TEXT,
  stack_json TEXT,
  PRIMARY KEY (thread_id, host, repository, number)
);
```

`apps/server/src/persistence/Migrations/050_ProjectionThreadPullRequests.ts:49`

```sql
CREATE INDEX IF NOT EXISTS idx_projection_thread_pull_requests_pr
ON projection_thread_pull_requests(host, repository, number);
```

### Migration 051_ProjectionThreadMessageContext

Source: `apps/server/src/persistence/Migrations/051_ProjectionThreadMessageContext.ts:1`.

`apps/server/src/persistence/Migrations/051_ProjectionThreadMessageContext.ts:11`

```sql
ALTER TABLE projection_thread_messages
ADD COLUMN context_json TEXT;
```

### Migration 052_ProjectionThreadTitleState

Source: `apps/server/src/persistence/Migrations/052_ProjectionThreadTitleState.ts:1`.

`apps/server/src/persistence/Migrations/052_ProjectionThreadTitleState.ts:6`

```sql
ALTER TABLE projection_threads ADD COLUMN title_state_json TEXT;
```

### Migration 053_PullRequestFilesViewed

Source: `apps/server/src/persistence/Migrations/053_PullRequestFilesViewed.ts:1`.

`apps/server/src/persistence/Migrations/053_PullRequestFilesViewed.ts:8`

```sql
CREATE TABLE IF NOT EXISTS pull_request_files_viewed (
  provider TEXT NOT NULL,
  host TEXT NOT NULL,
  repository TEXT NOT NULL,
  number INTEGER NOT NULL,
  viewer TEXT NOT NULL,
  path TEXT NOT NULL,
  revision TEXT,
  viewed_at TEXT NOT NULL,
  PRIMARY KEY (provider, host, repository, number, viewer, path)
) WITHOUT ROWID;
```

### Migration 054_ProjectionThreadsAutoSettleDisabledAt

Source: `apps/server/src/persistence/Migrations/054_ProjectionThreadsAutoSettleDisabledAt.ts:1`.

`apps/server/src/persistence/Migrations/054_ProjectionThreadsAutoSettleDisabledAt.ts:10`

```sql
ALTER TABLE projection_threads
ADD COLUMN auto_settle_disabled_at TEXT;
```

### Migration 055_OrchestrationV2

Source: `apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:1`.

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:19`

```sql
CREATE TABLE orchestration_v2_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  command_id TEXT,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  provider TEXT,
  raw_event_id TEXT,
  event_type TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:34`

```sql
CREATE INDEX orchestration_v2_events_command_idx ON orchestration_v2_events(command_id, sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:35`

```sql
CREATE INDEX orchestration_v2_events_thread_sequence_idx ON orchestration_v2_events(thread_id, sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:36`

```sql
CREATE INDEX orchestration_v2_events_thread_type_sequence_idx ON orchestration_v2_events(thread_id, event_type, sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:37`

```sql
CREATE INDEX orchestration_v2_events_run_sequence_idx ON orchestration_v2_events(run_id, sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:38`

```sql
CREATE INDEX orchestration_v2_events_node_sequence_idx ON orchestration_v2_events(node_id, sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:39`

```sql
CREATE INDEX orchestration_v2_events_raw_event_idx ON orchestration_v2_events(raw_event_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:41`

```sql
CREATE TABLE orchestration_v2_command_receipts (
  command_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  command_type TEXT NOT NULL,
  accepted_at TEXT NOT NULL,
  result_sequence INTEGER NOT NULL,
  status TEXT NOT NULL,
  error TEXT
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:52`

```sql
CREATE INDEX orchestration_v2_command_receipts_thread_sequence_idx ON orchestration_v2_command_receipts(thread_id, result_sequence);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:54`

```sql
CREATE TABLE orchestration_v2_projection_threads (
  thread_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  default_provider TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  active_provider_thread_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:70`

```sql
CREATE INDEX orchestration_v2_projection_threads_project_updated_idx ON orchestration_v2_projection_threads(project_id, updated_at);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:72`

```sql
CREATE TABLE orchestration_v2_projection_runs (
  run_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT,
  status TEXT NOT NULL,
  requested_at TEXT NOT NULL,
  completed_at TEXT,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:85`

```sql
CREATE UNIQUE INDEX orchestration_v2_projection_runs_thread_ordinal_idx ON orchestration_v2_projection_runs(thread_id, ordinal);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:86`

```sql
CREATE INDEX orchestration_v2_projection_runs_provider_thread_idx ON orchestration_v2_projection_runs(provider_thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:87`

```sql
CREATE INDEX orchestration_v2_projection_runs_thread_status_idx ON orchestration_v2_projection_runs(thread_id, status);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:89`

```sql
CREATE TABLE orchestration_v2_projection_run_attempts (
  attempt_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  attempt_ordinal INTEGER NOT NULL,
  root_node_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  provider_turn_id TEXT,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:103`

```sql
CREATE UNIQUE INDEX orchestration_v2_projection_run_attempts_run_ordinal_idx ON orchestration_v2_projection_run_attempts(run_id, attempt_ordinal);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:104`

```sql
CREATE INDEX orchestration_v2_projection_run_attempts_thread_idx ON orchestration_v2_projection_run_attempts(thread_id, run_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:106`

```sql
CREATE TABLE orchestration_v2_projection_nodes (
  node_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  parent_node_id TEXT,
  root_node_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  provider_thread_id TEXT,
  provider_turn_id TEXT,
  runtime_request_id TEXT,
  checkpoint_scope_id TEXT,
  started_at TEXT,
  completed_at TEXT,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:124`

```sql
CREATE INDEX orchestration_v2_projection_nodes_thread_run_idx ON orchestration_v2_projection_nodes(thread_id, run_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:125`

```sql
CREATE INDEX orchestration_v2_projection_nodes_parent_idx ON orchestration_v2_projection_nodes(parent_node_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:126`

```sql
CREATE INDEX orchestration_v2_projection_nodes_provider_turn_idx ON orchestration_v2_projection_nodes(provider_turn_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:128`

```sql
CREATE TABLE orchestration_v2_projection_provider_sessions (
  provider_session_id TEXT PRIMARY KEY,
  thread_id TEXT,
  provider TEXT NOT NULL,
  status TEXT NOT NULL,
  model TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:139`

```sql
CREATE INDEX orchestration_v2_projection_provider_sessions_thread_idx ON orchestration_v2_projection_provider_sessions(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:140`

```sql
CREATE INDEX orchestration_v2_projection_provider_sessions_provider_status_idx ON orchestration_v2_projection_provider_sessions(provider, status);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:142`

```sql
CREATE TABLE orchestration_v2_projection_provider_threads (
  provider_thread_id TEXT PRIMARY KEY,
  thread_id TEXT,
  owner_node_id TEXT,
  provider TEXT NOT NULL,
  provider_session_id TEXT,
  status TEXT NOT NULL,
  first_run_ordinal INTEGER,
  last_run_ordinal INTEGER,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:156`

```sql
CREATE INDEX orchestration_v2_projection_provider_threads_thread_idx ON orchestration_v2_projection_provider_threads(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:157`

```sql
CREATE INDEX orchestration_v2_projection_provider_threads_session_idx ON orchestration_v2_projection_provider_threads(provider_session_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:158`

```sql
CREATE INDEX orchestration_v2_projection_provider_threads_owner_idx ON orchestration_v2_projection_provider_threads(owner_node_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:160`

```sql
CREATE TABLE orchestration_v2_projection_provider_turns (
  provider_turn_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  run_attempt_id TEXT,
  ordinal INTEGER NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:174`

```sql
CREATE INDEX orchestration_v2_projection_provider_turns_thread_idx ON orchestration_v2_projection_provider_turns(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:175`

```sql
CREATE UNIQUE INDEX orchestration_v2_projection_provider_turns_thread_ordinal_idx ON orchestration_v2_projection_provider_turns(provider_thread_id, ordinal);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:177`

```sql
CREATE TABLE orchestration_v2_projection_runtime_requests (
  runtime_request_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  provider_turn_id TEXT,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  resolved_at TEXT,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:190`

```sql
CREATE INDEX orchestration_v2_projection_runtime_requests_thread_status_idx ON orchestration_v2_projection_runtime_requests(thread_id, status);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:191`

```sql
CREATE INDEX orchestration_v2_projection_runtime_requests_provider_turn_idx ON orchestration_v2_projection_runtime_requests(provider_turn_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:193`

```sql
CREATE TABLE orchestration_v2_projection_messages (
  message_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  role TEXT NOT NULL,
  streaming INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:206`

```sql
CREATE INDEX orchestration_v2_projection_messages_thread_created_idx ON orchestration_v2_projection_messages(thread_id, created_at, message_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:207`

```sql
CREATE INDEX orchestration_v2_projection_messages_run_idx ON orchestration_v2_projection_messages(run_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:208`

```sql
CREATE INDEX orchestration_v2_projection_messages_node_idx ON orchestration_v2_projection_messages(node_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:210`

```sql
CREATE TABLE orchestration_v2_projection_plans (
  plan_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:221`

```sql
CREATE INDEX orchestration_v2_projection_plans_thread_idx ON orchestration_v2_projection_plans(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:222`

```sql
CREATE INDEX orchestration_v2_projection_plans_run_idx ON orchestration_v2_projection_plans(run_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:224`

```sql
CREATE TABLE orchestration_v2_projection_turn_items (
  turn_item_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  provider_thread_id TEXT,
  provider_turn_id TEXT,
  parent_item_id TEXT,
  ordinal INTEGER NOT NULL,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:240`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_thread_ordinal_idx ON orchestration_v2_projection_turn_items(thread_id, ordinal, turn_item_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:241`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_run_ordinal_idx ON orchestration_v2_projection_turn_items(run_id, ordinal);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:242`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_node_ordinal_idx ON orchestration_v2_projection_turn_items(node_id, ordinal);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:243`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_provider_turn_idx ON orchestration_v2_projection_turn_items(provider_turn_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:245`

```sql
CREATE TABLE orchestration_v2_projection_checkpoint_scopes (
  scope_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  parent_scope_id TEXT,
  provider_thread_id TEXT,
  kind TEXT NOT NULL,
  ordinal_within_parent INTEGER NOT NULL,
  advances_app_run_count INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:260`

```sql
CREATE INDEX orchestration_v2_projection_checkpoint_scopes_thread_idx ON orchestration_v2_projection_checkpoint_scopes(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:261`

```sql
CREATE INDEX orchestration_v2_projection_checkpoint_scopes_parent_idx ON orchestration_v2_projection_checkpoint_scopes(parent_scope_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:263`

```sql
CREATE TABLE orchestration_v2_projection_checkpoints (
  checkpoint_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  parent_checkpoint_id TEXT,
  ordinal_within_scope INTEGER NOT NULL,
  app_run_ordinal INTEGER,
  status TEXT NOT NULL,
  captured_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:278`

```sql
CREATE UNIQUE INDEX orchestration_v2_projection_checkpoints_scope_ordinal_idx ON orchestration_v2_projection_checkpoints(scope_id, ordinal_within_scope);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:279`

```sql
CREATE INDEX orchestration_v2_projection_checkpoints_thread_idx ON orchestration_v2_projection_checkpoints(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:280`

```sql
CREATE INDEX orchestration_v2_projection_checkpoints_parent_idx ON orchestration_v2_projection_checkpoints(parent_checkpoint_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:282`

```sql
CREATE TABLE orchestration_v2_projection_context_handoffs (
  context_handoff_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  target_run_id TEXT NOT NULL,
  to_provider_thread_id TEXT NOT NULL,
  strategy TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:294`

```sql
CREATE INDEX orchestration_v2_projection_context_handoffs_thread_idx ON orchestration_v2_projection_context_handoffs(thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:295`

```sql
CREATE INDEX orchestration_v2_projection_context_handoffs_target_run_idx ON orchestration_v2_projection_context_handoffs(target_run_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:297`

```sql
CREATE TABLE orchestration_v2_projection_context_transfers (
  context_transfer_id TEXT PRIMARY KEY,
  source_thread_id TEXT NOT NULL,
  target_thread_id TEXT NOT NULL,
  target_run_id TEXT,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  source_provider TEXT,
  target_provider TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:311`

```sql
CREATE INDEX orchestration_v2_projection_context_transfers_source_thread_idx ON orchestration_v2_projection_context_transfers(source_thread_id);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:312`

```sql
CREATE INDEX orchestration_v2_projection_context_transfers_target_thread_idx ON orchestration_v2_projection_context_transfers(target_thread_id, status);
```

`apps/server/src/persistence/Migrations/055_OrchestrationV2.ts:313`

```sql
CREATE INDEX orchestration_v2_projection_context_transfers_target_run_idx ON orchestration_v2_projection_context_transfers(target_run_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:8`

```sql
ALTER TABLE orchestration_v2_events ADD COLUMN driver TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:9`

```sql
ALTER TABLE orchestration_v2_events ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:19`

```sql
CREATE INDEX orchestration_v2_events_instance_sequence_idx ON orchestration_v2_events(provider_instance_id, sequence);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:21`

```sql
ALTER TABLE orchestration_v2_projection_threads ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:30`

```sql
ALTER TABLE orchestration_v2_projection_runs ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:39`

```sql
ALTER TABLE orchestration_v2_projection_run_attempts ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:48`

```sql
ALTER TABLE orchestration_v2_projection_provider_sessions ADD COLUMN driver TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:49`

```sql
ALTER TABLE orchestration_v2_projection_provider_sessions ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:59`

```sql
CREATE INDEX orchestration_v2_projection_provider_sessions_instance_status_idx ON orchestration_v2_projection_provider_sessions(provider_instance_id, status);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:61`

```sql
ALTER TABLE orchestration_v2_projection_provider_threads ADD COLUMN driver TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:62`

```sql
ALTER TABLE orchestration_v2_projection_provider_threads ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:72`

```sql
CREATE INDEX orchestration_v2_projection_provider_threads_instance_status_idx ON orchestration_v2_projection_provider_threads(provider_instance_id, status);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:74`

```sql
ALTER TABLE orchestration_v2_projection_subagents ADD COLUMN driver TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:75`

```sql
ALTER TABLE orchestration_v2_projection_subagents ADD COLUMN provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:86`

```sql
ALTER TABLE orchestration_v2_projection_context_transfers ADD COLUMN source_provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:87`

```sql
ALTER TABLE orchestration_v2_projection_context_transfers ADD COLUMN target_provider_instance_id TEXT;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:101`

```sql
CREATE TABLE orchestration_v2_effect_outbox (
  effect_id TEXT PRIMARY KEY,
  command_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  effect_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
  attempt_count INTEGER NOT NULL DEFAULT 0,
  available_at TEXT NOT NULL,
  lease_owner TEXT,
  lease_expires_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  last_error TEXT
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:119`

```sql
CREATE INDEX orchestration_v2_effect_outbox_claim_idx ON orchestration_v2_effect_outbox(status, available_at, lease_expires_at, created_at);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:120`

```sql
CREATE INDEX orchestration_v2_effect_outbox_command_idx ON orchestration_v2_effect_outbox(command_id, effect_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:122`

```sql
CREATE TABLE orchestration_v2_turn_item_positions (
  thread_id TEXT NOT NULL,
  turn_item_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY (thread_id, turn_item_id),
  UNIQUE (thread_id, ordinal)
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Foundation.ts:150`

```sql
CREATE TABLE orchestration_v2_projection_metadata (
  projection_name TEXT PRIMARY KEY,
  schema_version INTEGER NOT NULL,
  last_sequence INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ThreadLaunchWorkflows.ts:6`

```sql
CREATE TABLE IF NOT EXISTS orchestration_v2_thread_launch_workflows (
  command_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  status TEXT NOT NULL,
  title TEXT NOT NULL,
  worktree_path TEXT,
  branch TEXT,
  setup_committed INTEGER NOT NULL DEFAULT 0,
  thread_committed INTEGER NOT NULL DEFAULT 0,
  message_committed INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ProviderSessionBindings.ts:7`

```sql
CREATE TABLE orchestration_v2_projection_provider_session_bindings (
  provider_session_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  PRIMARY KEY (provider_session_id, thread_id)
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ProviderSessionBindings.ts:23`

```sql
CREATE INDEX orchestration_v2_projection_provider_session_bindings_thread_idx
ON orchestration_v2_projection_provider_session_bindings(thread_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Subagents.ts:7`

```sql
CREATE TABLE orchestration_v2_projection_subagents (
  subagent_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  parent_node_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT,
  child_thread_id TEXT,
  origin TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Subagents.ts:24`

```sql
CREATE INDEX orchestration_v2_projection_subagents_thread_idx ON orchestration_v2_projection_subagents(thread_id, started_at, subagent_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Subagents.ts:25`

```sql
CREATE INDEX orchestration_v2_projection_subagents_parent_node_idx ON orchestration_v2_projection_subagents(parent_node_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Subagents.ts:26`

```sql
CREATE INDEX orchestration_v2_projection_subagents_provider_thread_idx ON orchestration_v2_projection_subagents(provider_thread_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/Subagents.ts:27`

```sql
CREATE INDEX orchestration_v2_projection_subagents_child_thread_idx ON orchestration_v2_projection_subagents(child_thread_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:7`

```sql
CREATE TABLE orchestration_v2_effect_outbox_next (
  effect_id TEXT PRIMARY KEY,
  command_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  effect_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (
    status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')
  ),
  attempt_count INTEGER NOT NULL DEFAULT 0,
  available_at TEXT NOT NULL,
  lease_owner TEXT,
  lease_expires_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  last_error TEXT
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:61`

```sql
DROP TABLE orchestration_v2_effect_outbox;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:62`

```sql
ALTER TABLE orchestration_v2_effect_outbox_next
RENAME TO orchestration_v2_effect_outbox;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:66`

```sql
CREATE INDEX orchestration_v2_effect_outbox_claim_idx
ON orchestration_v2_effect_outbox(status, available_at, lease_expires_at, created_at);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:70`

```sql
CREATE INDEX orchestration_v2_effect_outbox_command_idx
ON orchestration_v2_effect_outbox(command_id, effect_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/EffectCancellation.ts:74`

```sql
CREATE INDEX orchestration_v2_effect_outbox_thread_status_idx
ON orchestration_v2_effect_outbox(thread_id, status, effect_type);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ScheduledTasks.ts:7`

```sql
CREATE TABLE IF NOT EXISTS scheduled_tasks (
  task_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  prompt TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  schedule_json TEXT NOT NULL,
  project_id TEXT NOT NULL,
  thread_id TEXT,
  workspace_strategy_json TEXT NOT NULL,
  model_selection_json TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  created_by TEXT NOT NULL,
  creation_source TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  next_run_at TEXT,
  last_run_at TEXT,
  last_run_status TEXT NOT NULL,
  last_run_error TEXT,
  run_count INTEGER NOT NULL
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ScheduledTasks.ts:32`

```sql
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
ON scheduled_tasks(enabled, next_run_at)
WHERE enabled = 1 AND next_run_at IS NOT NULL;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ScheduledTasks.ts:38`

```sql
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_project
ON scheduled_tasks(project_id, updated_at);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ApplicationEventSource.ts:36`

```sql
ALTER TABLE orchestration_events
ADD COLUMN application_event_version INTEGER NOT NULL DEFAULT 1;
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ApplicationEventSource.ts:40`

```sql
CREATE INDEX idx_orchestration_events_application_sequence
ON orchestration_events(application_event_version, sequence);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ApplicationEventSource.ts:44`

```sql
ALTER TABLE orchestration_command_receipts
ADD COLUMN command_type TEXT NOT NULL DEFAULT 'legacy';
```

`apps/server/src/persistence/Migrations/OrchestrationV2/LegacyV1ImportState.ts:7`

```sql
CREATE TABLE orchestration_v2_legacy_imports (
  thread_id TEXT PRIMARY KEY,
  source_updated_at TEXT NOT NULL,
  shell_imported_at TEXT NOT NULL,
  transcript_imported_at TEXT,
  imported_message_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/LegacyV1ImportState.ts:18`

```sql
CREATE INDEX orchestration_v2_legacy_imports_pending_transcript_idx
ON orchestration_v2_legacy_imports(transcript_imported_at, shell_imported_at, thread_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ApplicationEventSequenceIndexes.ts:7`

```sql
CREATE INDEX idx_orchestration_events_application_high_water
ON orchestration_events(sequence)
WHERE aggregate_kind = 'project'
  OR (application_event_version = 2 AND aggregate_kind = 'thread');
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ApplicationEventSequenceIndexes.ts:13`

```sql
CREATE INDEX idx_orchestration_events_agent_stream_sequence
ON orchestration_events(stream_id, sequence)
WHERE application_event_version = 2 AND aggregate_kind = 'thread';
```

`apps/server/src/persistence/Migrations/OrchestrationV2/RecoveryIndexes.ts:7`

```sql
CREATE INDEX orchestration_events_v2_created_threads_idx
ON orchestration_events(stream_id)
WHERE application_event_version = 2
  AND aggregate_kind = 'thread'
  AND event_type = 'thread.created';
```

`apps/server/src/persistence/Migrations/OrchestrationV2/RecoveryIndexes.ts:14`

```sql
CREATE INDEX orchestration_v2_projection_runs_recovery_idx
ON orchestration_v2_projection_runs(status, thread_id)
WHERE status IN ('queued', 'preparing', 'starting', 'running', 'waiting');
```

`apps/server/src/persistence/Migrations/OrchestrationV2/RecoveryIndexes.ts:19`

```sql
CREATE INDEX orchestration_v2_projection_requests_recovery_idx
ON orchestration_v2_projection_runtime_requests(thread_id)
WHERE status = 'pending';
```

`apps/server/src/persistence/Migrations/OrchestrationV2/RecoveryIndexes.ts:24`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_recovery_idx
ON orchestration_v2_projection_turn_items(thread_id)
WHERE type IN ('command_execution', 'dynamic_tool', 'subagent')
  AND status IN ('pending', 'running', 'waiting');
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ShellIndexes.ts:7`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_thread_run_idx
ON orchestration_v2_projection_turn_items(thread_id, run_id);
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ShellIndexes.ts:12`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_shell_pending_idx
ON orchestration_v2_projection_turn_items(thread_id, run_id)
WHERE type IN ('command_execution', 'dynamic_tool', 'subagent')
  AND status NOT IN ('completed', 'interrupted', 'failed', 'cancelled');
```

`apps/server/src/persistence/Migrations/OrchestrationV2/ShellIndexes.ts:18`

```sql
CREATE INDEX orchestration_v2_projection_messages_latest_user_idx
ON orchestration_v2_projection_messages(thread_id, updated_at DESC, message_id DESC)
WHERE role = 'user';
```

### Migration 056_RemoveRedundantProjectionIndexes

Source: `apps/server/src/persistence/Migrations/056_RemoveRedundantProjectionIndexes.ts:1`.

`apps/server/src/persistence/Migrations/056_RemoveRedundantProjectionIndexes.ts:8`

```sql
DROP INDEX IF EXISTS idx_projection_threads_project_id;
```

`apps/server/src/persistence/Migrations/056_RemoveRedundantProjectionIndexes.ts:9`

```sql
DROP INDEX IF EXISTS idx_projection_thread_messages_thread_created;
```

`apps/server/src/persistence/Migrations/056_RemoveRedundantProjectionIndexes.ts:10`

```sql
DROP INDEX IF EXISTS idx_projection_thread_activities_thread_sequence;
```

`apps/server/src/persistence/Migrations/056_RemoveRedundantProjectionIndexes.ts:11`

```sql
DROP INDEX IF EXISTS idx_projection_turns_thread_requested;
```

### Migration 057_ScheduledTaskWebhooks

Source: `apps/server/src/persistence/Migrations/057_ScheduledTaskWebhooks.ts:1`.

`apps/server/src/persistence/Migrations/057_ScheduledTaskWebhooks.ts:8`

```sql
ALTER TABLE scheduled_tasks ADD COLUMN webhook_token TEXT;
```

`apps/server/src/persistence/Migrations/057_ScheduledTaskWebhooks.ts:9`

```sql
ALTER TABLE scheduled_tasks ADD COLUMN webhook_secret TEXT;
```

`apps/server/src/persistence/Migrations/057_ScheduledTaskWebhooks.ts:11`

```sql
CREATE TABLE IF NOT EXISTS scheduled_task_webhook_deliveries (
  delivery_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  received_at TEXT NOT NULL,
  method TEXT NOT NULL,
  query TEXT NOT NULL,
  headers_json TEXT NOT NULL,
  body TEXT NOT NULL,
  body_bytes INTEGER NOT NULL,
  body_truncated INTEGER NOT NULL,
  outcome TEXT NOT NULL,
  signature_verified INTEGER NOT NULL,
  missing_fields_json TEXT NOT NULL,
  rendered_prompt TEXT,
  error TEXT
);
```

`apps/server/src/persistence/Migrations/057_ScheduledTaskWebhooks.ts:30`

```sql
CREATE INDEX IF NOT EXISTS idx_scheduled_task_webhook_deliveries_task
ON scheduled_task_webhook_deliveries(task_id, received_at);
```

### Migration 058_WebhookRelayDeliveries

Source: `apps/server/src/persistence/Migrations/058_WebhookRelayDeliveries.ts:1`.

`apps/server/src/persistence/Migrations/058_WebhookRelayDeliveries.ts:8`

```sql
CREATE TABLE IF NOT EXISTS scheduled_task_webhook_relay_deliveries (
  relay_delivery_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  seen_at TEXT NOT NULL
);
```

`apps/server/src/persistence/Migrations/058_WebhookRelayDeliveries.ts:16`

```sql
CREATE INDEX IF NOT EXISTS idx_scheduled_task_webhook_relay_deliveries_seen
ON scheduled_task_webhook_relay_deliveries(seen_at);
```

### Migration 059_McpAppModelContext

Source: `apps/server/src/persistence/Migrations/059_McpAppModelContext.ts:1`.

`apps/server/src/persistence/Migrations/059_McpAppModelContext.ts:7`

```sql
CREATE TABLE IF NOT EXISTS mcp_app_model_context (
  thread_id TEXT NOT NULL,
  item_id TEXT NOT NULL,
  server TEXT NOT NULL,
  tool TEXT NOT NULL,
  text TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (thread_id, item_id)
);
```

### Migration 060_ThreadSnapshotWindowIndexes

Source: `apps/server/src/persistence/Migrations/060_ThreadSnapshotWindowIndexes.ts:1`.

`apps/server/src/persistence/Migrations/060_ThreadSnapshotWindowIndexes.ts:7`

```sql
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_user_message_idx
ON orchestration_v2_projection_turn_items(thread_id, ordinal, turn_item_id)
WHERE type = 'user_message';
```

`apps/server/src/persistence/Migrations/060_ThreadSnapshotWindowIndexes.ts:12`

```sql
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_live_idx
ON orchestration_v2_projection_nodes(thread_id)
WHERE status IN ('pending', 'starting', 'running', 'waiting');
```

### Migration 061_PullRequestReviewGuides

Source: `apps/server/src/persistence/Migrations/061_PullRequestReviewGuides.ts:1`.

`apps/server/src/persistence/Migrations/061_PullRequestReviewGuides.ts:7`

```sql
CREATE TABLE IF NOT EXISTS pull_request_review_guides (
  host TEXT NOT NULL,
  repository TEXT NOT NULL,
  number INTEGER NOT NULL,
  guide_json TEXT NOT NULL,
  PRIMARY KEY (host, repository, number)
) WITHOUT ROWID;
```

### Migration 062_ThreadFailureLookupIndex

Source: `apps/server/src/persistence/Migrations/062_ThreadFailureLookupIndex.ts:1`.

`apps/server/src/persistence/Migrations/062_ThreadFailureLookupIndex.ts:7`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_failed_error_idx
ON orchestration_v2_projection_turn_items(
  thread_id, run_id, node_id, updated_at DESC, ordinal DESC, turn_item_id DESC
)
WHERE type = 'error' AND status = 'failed';
```

### Migration 063_DropUnusedV1Projections

Source: `apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:1`.

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:8`

```sql
DROP TABLE IF EXISTS projection_thread_activities;
```

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:9`

```sql
DROP TABLE IF EXISTS projection_turns;
```

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:10`

```sql
DROP TABLE IF EXISTS projection_thread_proposed_plans;
```

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:11`

```sql
DROP TABLE IF EXISTS projection_pending_approvals;
```

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:12`

```sql
DROP TABLE IF EXISTS projection_state;
```

`apps/server/src/persistence/Migrations/063_DropUnusedV1Projections.ts:13`

```sql
DROP TABLE IF EXISTS checkpoint_diff_blobs;
```

### Migration 064_CanonicalizeThreadPullRequestLinks

Source: `apps/server/src/persistence/Migrations/064_CanonicalizeThreadPullRequestLinks.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 065_CodexUsageNativeTurnIndex

Source: `apps/server/src/persistence/Migrations/065_CodexUsageNativeTurnIndex.ts:1`.

`apps/server/src/persistence/Migrations/065_CodexUsageNativeTurnIndex.ts:7`

```sql
CREATE INDEX orchestration_v2_provider_turns_codex_native_id_idx
ON orchestration_v2_projection_provider_turns (
  json_extract(payload_json, '$.nativeTurnRef.nativeId')
)
WHERE json_extract(payload_json, '$.nativeTurnRef.driver') = 'codex';
```

### Migration 066_RepairClientSettlementTimestamps

Source: `apps/server/src/persistence/Migrations/066_RepairClientSettlementTimestamps.ts:1`.

Also performs or prepares data operations: UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

No static CREATE/ALTER/DROP template; this migration repairs/backfills data or uses conditional/dynamic SQL.

### Migration 067_WebhookDedupe

Source: `apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:1`.

Also performs or prepares data operations: DELETE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:6`

```sql
ALTER TABLE scheduled_tasks ADD COLUMN last_run_webhook_delivery_id TEXT;
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:7`

```sql
CREATE TABLE scheduled_task_webhook_replay_keys (
    task_id TEXT NOT NULL REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE,
    replay_key TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, replay_key)
  );
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:14`

```sql
CREATE INDEX idx_webhook_replay_expiry ON scheduled_task_webhook_replay_keys(expires_at);
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:15`

```sql
CREATE INDEX idx_webhook_replay_delivery ON scheduled_task_webhook_replay_keys(delivery_id);
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:16`

```sql
CREATE TABLE scheduled_task_webhook_pending (
    delivery_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE,
    coalesce_key TEXT NOT NULL,
    due_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    task_json TEXT NOT NULL,
    request_json TEXT NOT NULL,
    delivery_ids_json TEXT NOT NULL
  );
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:26`

```sql
CREATE INDEX idx_webhook_pending_due ON scheduled_task_webhook_pending(state, due_at);
```

`apps/server/src/persistence/Migrations/067_WebhookDedupe.ts:27`

```sql
CREATE UNIQUE INDEX idx_webhook_pending_key
    ON scheduled_task_webhook_pending(task_id, coalesce_key) WHERE state = 'pending';
```

### Migration 068_ShellStopRequestIndex

Source: `apps/server/src/persistence/Migrations/068_ShellStopRequestIndex.ts:1`.

`apps/server/src/persistence/Migrations/068_ShellStopRequestIndex.ts:7`

```sql
CREATE INDEX orchestration_v2_projection_turn_items_stop_request_idx
ON orchestration_v2_projection_turn_items(thread_id, run_id)
WHERE type = 'run_interrupt_request';
```

### Migration 069_ThreadMessageSearch

Source: `apps/server/src/persistence/Migrations/069_ThreadMessageSearch.ts:1`.

Also performs or prepares data operations: DELETE, INSERT, UPDATE. Preserve the migration's backfill/repair semantics when reusing an existing DB.

`apps/server/src/persistence/Migrations/069_ThreadMessageSearch.ts:22`

```sql
CREATE VIRTUAL TABLE orchestration_v2_message_search USING fts5(
  text,
  content = '',
  contentless_delete = 1,
  tokenize = 'trigram remove_diacritics 1'
);
```

`apps/server/src/persistence/Migrations/069_ThreadMessageSearch.ts:42`

```sql
CREATE TRIGGER orchestration_v2_message_search_insert
AFTER INSERT ON orchestration_v2_projection_messages
WHEN ${indexedMessageCondition(sql, "new")}
BEGIN
  INSERT INTO orchestration_v2_message_search (rowid, text)
  SELECT new.rowid, ${indexedMessageText(sql, "new.payload_json")}
  WHERE json_extract(new.payload_json, '$.text') IS NOT NULL;
END;
```

`apps/server/src/persistence/Migrations/069_ThreadMessageSearch.ts:56`

```sql
CREATE TRIGGER orchestration_v2_message_search_update
AFTER UPDATE OF payload_json, streaming, role ON orchestration_v2_projection_messages
WHEN (old.streaming = 0 OR new.streaming = 0)
  AND (
    old.streaming IS NOT new.streaming
    OR old.role IS NOT new.role
    OR json_extract(old.payload_json, '$.text') IS NOT json_extract(new.payload_json, '$.text')
    OR json_extract(old.payload_json, '$.createdBy')
      IS NOT json_extract(new.payload_json, '$.createdBy')
  )
BEGIN
  DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid AND old.streaming = 0;
  INSERT INTO orchestration_v2_message_search (rowid, text)
  SELECT new.rowid, ${indexedMessageText(sql, "new.payload_json")}
  WHERE ${indexedMessageCondition(sql, "new")}
    AND json_extract(new.payload_json, '$.text') IS NOT NULL;
END;
```

`apps/server/src/persistence/Migrations/069_ThreadMessageSearch.ts:75`

```sql
CREATE TRIGGER orchestration_v2_message_search_delete
AFTER DELETE ON orchestration_v2_projection_messages
WHEN old.streaming = 0
BEGIN
  DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid;
END;
```

## Appendix E. Exact payload-dependent permission rules and verification

`requiredScopesForServerSettingsPatch` examines only keys whose value is present. Any of `providers`, `providerInstances`, `usageLimitSources` requires providers:manage. Any other present key requires settings:write. Both kinds require both permissions. An empty patch requires settings:write. With a providerInstanceMutation and an otherwise empty patch, the RPC requires only providers:manage; with a nonempty patch it adds providers:manage to the patch requirements. Sources: `packages/contracts/src/settings.ts:1577`; `apps/server/src/auth/RpcAuthorization.ts:240`.

`requiredScopesForProjectMutation`: project.delete or any mutation without scripts → orchestration:operate only. Supplying scripts on a non-delete mutation adds settings:write. This rule does not infer filesystem permission from a workspace path. Source: `packages/contracts/src/settings.ts:1597`.

HTTP scope map beyond the RPC matrix:

| Operation | Permission / authentication |
|---|---|
| orchestration shell/thread/transcript/bounded/history | orchestration:read, protocol header 2 |
| GET /api/projects | orchestration:read |
| POST /api/projects/mutate | project mutation rules above |
| POST /api/pull-requests/diff | orchestration:read |
| pairing-token, pairing-links/revoke, clients/revoke, clients/revoke-others | access:write; requested pairing grant must be a nonempty, duplicate-free subset of the caller's grant |
| pairing-links and clients lists | access:read |
| connect link-proof, relay-config, unlink, preferences | relay:write |
| connect link-state, reachability | relay:read |
| connect mint-credential aliases; t3-connect/health | signed cloud proof validation, not ordinary session authentication |
| POST /api/observability/v1/traces | authenticated orchestration:operate |
| signed asset and upload URLs | possession of valid resource/expiry-bound token; permissions checked at mint time |
| guest routes | share credential/session and share state; no owner RPC scope inheritance |

Sources: `apps/server/src/project/http.ts:45`; `apps/server/src/cloud/http.ts:115`; `apps/server/src/auth/http.ts:448`; `apps/server/src/http.ts:288`; `apps/server/src/pullRequest/http.ts:19`.

### Implementation acceptance checklist

1. Native client exchanges the bootstrap token using form encoding, validates descriptor/auth, mints an audience-correct ticket and opens protocol-2 WS.
2. Unary calls preserve request IDs, schema shapes, void results, tagged errors and granular permission denial.
3. Streaming calls send nonempty chunks, wait for ACK as appropriate, remain responsive to JSON/control ping, and cancel independently on Interrupt.
4. Project + thread commands append into one global log; duplicate command IDs do not repeat effects; snapshots are consistent with their high-water mark.
5. Resume tolerates gaps/overlap and can replace state with a snapshot. Compact encodings are strictly opt-in and preserve arbitrary tool JSON.
6. Bounded-history cursors page history independently from live sequence cursors; missing deduped bodies trigger a resnapshot.
7. Provider adapters preserve native identity and request/response lifetimes; process completion and background-work completion remain distinct.

Verification performed for this document: read-only source inspection at the pinned commit, cross-check of all 203 served method names / 31 stream flags / static-and-dynamic permission classifications against lib-t3-rust's extracted fixture, and inline source/schema extraction. No server was launched; no provider authentication or live end-to-end wire test was run. Synthetic fixture values in section 3 are expressly labeled. The installed Claude SDK/CLI subprocess protocol remains the principal external-version evidence gap; see section 6. DB import/backfill equivalence was not tested.

### Verified source index

The following references were located directly in the restored checkout or pinned Git tree. They complement the inline citations, especially for runtime implementation details beyond the wire schema.

- `apps/server/src/orchestration-v2/ProjectionStore.ts:654` — `ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION =`.
- `apps/server/src/orchestration-v2/ProjectionStore.ts:547` — `getThreadProjection`.
- `apps/server/src/orchestration-v2/ProjectionStore.ts:469` — `apply:`.
- `apps/server/src/orchestration-v2/EventSink.ts:275` — `withTransaction`.
- `apps/server/src/orchestration-v2/EventSink.ts:251` — `publishCommitted`.
- `apps/server/src/orchestration-v2/EventSink.ts:353` — `const applyStoredEvents`.
- `apps/server/src/orchestration-v2/EventSink.ts:138` — `commitCommand:`.
- `apps/server/src/persistence/OrchestrationEventStore.ts:49` — `appendAgentEvents:`.
- `apps/server/src/persistence/OrchestrationEventStore.ts:94` — `latestApplicationSequence`.
- `apps/server/src/persistence/OrchestrationEventStore.ts:111` — `streamProjectedApplicationEvents`.
- `apps/server/src/orchestration-v2/ThreadStream.ts:15` — `THREAD_RESUME_MAX_REPLAY_EVENTS`.
- `apps/server/src/orchestration-v2/ThreadStream.ts:94` — `decideThreadResume`.
- `apps/server/src/orchestration-v2/ThreadManagementService.ts:457` — `streamStoredEventsFrom`.
- `apps/server/src/orchestration-v2/ThreadManagementService.ts:370` — `getThreadSnapshotWindow`.
- `apps/server/src/terminal/OutputProtocol.ts:18` — `window`.
- `apps/server/src/terminal/OutputProtocol.ts:34` — `Ack`.
- `apps/server/src/vcs/GitVcsDriver.ts:746` — `checkpoint`.
- `apps/server/src/vcs/GitVcsDriver.ts:1151` — `update-ref`.
- `apps/server/src/vcs/GitVcsDriver.ts:1108` — `write-tree`.
- `apps/server/src/os-jank.ts:123` — `resolveBaseDir`.
- `apps/server/src/serverSettings.ts:42` — `writeFile`.
- `apps/server/src/serverSettings.ts:123` — `settingsPath`.
- `apps/server/src/pullRequest/http.ts:1` — supporting native transport / VCS / HTTP implementation.
- `apps/server/src/vcs/GitVcsDriver.ts:1` — supporting native transport / VCS / HTTP implementation.
- `apps/server/src/vcs/GitVcsDriverCore.ts:1` — supporting native transport / VCS / HTTP implementation.
- `packages/effect-codex-app-server/src/client.ts:1` — supporting native transport / VCS / HTTP implementation.
- `packages/effect-codex-app-server/src/schema.ts:1` — supporting native transport / VCS / HTTP implementation.
