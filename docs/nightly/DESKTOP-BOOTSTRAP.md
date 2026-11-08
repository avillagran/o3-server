# Desktop bootstrap foundation

Reference: nightly 4288, `32a0531748c44b25072d7495ce0b9bd7e9450a87`.

The foundation supplies the envelope codec, inherited-descriptor reader, rotating
token verifier and process-local desktop grant store. The following startup and
bearer integration is a separate checkpoint; full desktop authentication and the
client-side launcher remain incomplete.

`tools/generate-desktop-bootstrap-fixtures.mjs` executes the native envelope
schema and token module, then imports the complete native bootstrap reader with
its actual `HostProcess` and JSON-codec dependencies. It produces 44 token cases,
451 verification cases, 241 envelope cases and 13 descriptor-reader cases. Input
files, FIFOs and synthetic secrets exist only in a private networkless container.

The reader compares first-line, EOF, CR/LF, malformed raw UTF-8, invalid JSON,
invalid schema, unavailable descriptors and timeout behavior. Native Node rejects
a negative descriptor before `fstat`, producing a stat error; a valid but closed
descriptor instead yields no envelope. Reopening a regular-file descriptor leaves
the launcher's original descriptor open; the native direct-socket fallback owns
and closes its descriptor. Errors omit input contents. There is no invented
envelope-size cap. Rust JSON's escaped lone-surrogate limitation remains open.

`tools/generate-desktop-grant-fixtures.mjs` executes the complete native
`PairingGrantStore` constructor and consume methods. Only configuration,
repository and clock services are replaced with private fixtures; native grant
logic is unchanged. Native scope declarations are read from the pinned contract.
Its 1,350 scenarios compare missing/empty/token-only/rotating configurations,
expiry boundaries, repeated consumption, window offsets and requested/required
scope decisions. The fake repository records fallback calls, so an accidentally
persisted or consumed desktop seed cannot be hidden by the result comparison.

Token-only seeds are reusable for 24 hours. Rotating credentials accept the
previous, current or next 12-hour window, and produce a grant expiring 12 hours
after consumption. An explicitly configured secret suppresses legacy seeding.
The native empty-secret `InvalidKey` defect is retained. Native rotating grants
precede grant-store scope checks; the exchange service must still intersect
requested OAuth scopes before issuing access. Grant expiry does not imply the
same expiry for an issued bearer session.

Credentials have no serialization or debug implementation. Envelopes have no
serialization implementation and redact their debug representation. Production
callers must keep these values out of configuration projections, journals,
pairing tables and diagnostics.

## Startup and bearer integration

`--bootstrap-fd` takes precedence over `T3CODE_BOOTSTRAP_FD`. The first envelope is
read before profile discovery or persistent state creation. CLI and environment
mode, host, port and home override their envelope defaults. Explicit web mode
still accepts the provided desktop seed, matching the native grant store's
independence from the advertised bootstrap methods. Help, version, diagnostics,
auth/import/service commands and the private SSH helper bypass envelope intake.

Listener mode and process-local credentials are installed together. The actual
OAuth HTTP route accepts reusable legacy or rotating desktop credentials and
atomically replaces active desktop bearer sessions. Unrelated subjects remain
valid; the seed is never inserted into the pairing-link table. Trusted-peer
managed-policy checks still precede this exchange. Requested scopes use the
complete native OAuth parser and actual grant-scope schema as a 398-case oracle.
Issued scopes preserve requested order, remove duplicates and intersect the
grant. Rejected scopes do not spend ordinary one-time pairing links or revoke
the current desktop session. Until proof-bound issuance is implemented, a DPoP
header is rejected rather than silently producing a bearer credential.
Native cookie/DPoP issuance,
the complete OAuth parameter/error taxonomy and all client-policy consumers
remain separate work; the descriptor does not advertise unimplemented session
methods.

On Unix, startup captures the launcher parent before envelope intake. A desktop
server with an envelope and parent greater than one checks every two seconds and
shuts down on any parent change, including subreaper adoption. The shutdown fence
is entered before transports close, preserving durable runs. The physical test
starts a real private launcher/server pair, replaces the parent and checks both
runtime-registration removal and actual process exit using a stable pidfd.
Closing the launcher's stderr pipe while it remains alive must not end the
server. Serving diagnostics ignore broken-pipe writes, with a physical test
closing the pipe before supplying the envelope to avoid a startup race.

Browser/telemetry/control channels, resource monitoring, OTLP and bootstrap-driven
Tailscale Serve still need dedicated consumers. Requests for those channels fail
explicitly before profile creation. Existing headless service launches are not
silently relabeled as desktop-managed processes. The client must implement the
real inherited-FD launcher flow before that integration can be claimed.

Run Rust comparisons through `tools/test-scheduled-modes --unit-filter
desktop_bootstrap`, with `--default-features` for the other feature mode. Native
generators also run only inside private Podman containers with read-only source,
private home/tmp and no host credentials, display, devices or network.
