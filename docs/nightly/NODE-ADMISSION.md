# Explicit Node runtime admission

Native browser and speech resources require Node 26.13.0. Production owners
receive an explicit sealed executable; they do not discover Node through PATH.
The shared private-harness receipt names are `O3_ADMITTED_NODE`,
`O3_ADMITTED_NODE_SHA256` and `O3_ADMITTED_NODE_VERSION`.

`python3 tools/admit-node --node /absolute/path/to/node` opens every path
component without following links, requires a bounded regular unprivileged
executable, hashes its bytes and verifies its version in a private process. The
probe executes an owned immutable copy with a private HOME and cleared
environment. Source identity and bytes are checked again before the receipt is
returned. Host-side admission uses rootless Podman, read-only source and binary
mounts, no network and bounded resources. Genuine disposable GitHub Docker CI
can perform the same probe directly. Tool acquisition precedes this phase.

The CI job acquires exactly Node 26.13.0, then exports the three receipt fields.
The local Rust runner accepts `--node` or the explicit receipt path. Its legacy
tool-discovery fallback is used only during harness admission; production
workers never use it. Under the compilation lock, the runner copies the admitted
executable into its immutable runtime snapshot, records the SHA-256 and size and
passes the copied path explicitly to each private test container. Native child
processes can therefore reuse `process.execPath` without depending on a mutable
tool cache.

`tools/node_admission.py` exposes `admit`, `snapshot` and `environment`. Snapshot
destinations require a new owned parent directory; unknown destinations are
retained. Snapshots contain only the Node executable, not npm or ambient user
packages. Version probes and fixtures retain their private homes without tree
deletion. Receipts bind the admitted bytes and version; complete tool acquisition,
engine packages, model provenance and product capabilities remain separate
owner responsibilities.
