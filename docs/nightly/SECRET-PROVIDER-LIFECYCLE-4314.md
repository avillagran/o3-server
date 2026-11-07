# Secret-provider entry lifecycle

Reference: T3 Code nightly4314,
`83a3674ffd84ffb33de768210c806cb1ac53fd80`.

`secretProviders` patches merge by ID. Each supplied non-null entry replaces
that entry completely; null removes it; an empty map preserves existing entries.
The server validates the actual write envelope and the loaded secret driver's
config guard before changing settings or private slots. Stored envelopes use
stored-field decoding, preserving labels that exceed newer write-only limits.
An existing unknown kind keeps its opaque config on disk; its config is removed
from public settings. New writes of unloaded kinds are rejected.

Older clients may echo a reference as the redacted marker. The server restores
that reference before managed-setting and provider vetting, then clears
references to actually removed providers after vetting. Source-control,
usage-limit, device-host and provider-config fields become empty; provider
environment variables holding those references are removed. Other providers and
references to IDs that were never configured remain intact.

The pure native helper consumes decoded settings. Shared-profile environment
records encode their reference in a separate `secret` member. The physical
bridge transforms only the actual shared environment field through its closed
schema before cleanup and encodes it again afterward. It does not interpret
opaque driver config or infer an owner from a driver label.

Provider-only edits require `providers:manage`. Removing a provider also clears
settings fields and therefore requires `settings:write`. Mixed patches retain
the union of permissions. The actual authenticated RPC path enforces this split.

The source oracle executes the whole pinned patch, credential echo, removed
reference and loaded-kind guard methods with actual contracts and canonical
Effect4.1.2. It records eight patch traces,16 config-guard decisions and eight
permission decisions. Physical tests compare the supported credential groups,
retain unknown config privately, check legacy reference echo and verify rejected
writes leave both settings bytes and owned private slots unchanged. This is not
a whole-service oracle or a claim of cloud/local audio activation. The separate
speech foundation adds the cloud group through the same physical boundaries.

All runtime fixtures are private and retained. No test removes a filesystem
tree, invokes a real vault, installs a provider or runs a paid model.
