# Desktop bootstrap foundation

Reference: nightly 4288, `32a0531748c44b25072d7495ce0b9bd7e9450a87`.

This checkpoint supplies the envelope codec, inherited-descriptor reader, rotating
token verifier and process-local desktop grant store. Startup configuration,
OAuth exchange, launcher ownership/watch and desktop launcher integration remain
pending. These modules do not advertise a completed desktop authentication flow.

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

Run Rust comparisons through `tools/test-scheduled-modes --unit-filter
desktop_bootstrap`, with `--default-features` for the other feature mode. Native
generators also run only inside private Podman containers with read-only source,
private home/tmp and no host credentials, display, devices or network.
