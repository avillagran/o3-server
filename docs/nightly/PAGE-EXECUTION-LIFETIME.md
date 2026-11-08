# Native execution ownership after a caller deadline

A metadata caller deadline ends observation, not the admitted native execution.
The previous ControlledPage adapter awaited the deadline-returning worker API,
which could end RunContext while the worker still retained its wire tombstone.

`Worker::invoke_execution` now separates transport admission from completion.
Before admission, an owned mutex gate linearizes queued cancellation with the
actor committing its frame. An expired ninth request cannot later reach Node.
After admission, the future waits for the actual native result or the worker's
terminal authority fence. It does not extend the timeout sent to the native
operation. The eight active slots, existing owner-control lane and original
1,000 ms foreground host-response grace remain unchanged.

ControlledPage uses the retained execution API for owner-constructed metadata.
Its outer caller still receives the original deadline promptly; the admitted
callback and its generation-change finally block remain owned until actual
completion. Native readonly status/snapshot/waitFor can use the separate observe
entry under human control. Mutators cannot select that entry. These are trusted
owner APIs, not MCP argument-driven driver or timeout selectors.

## Evidence

`/var/tmp/o3-server/page-execution-terminal-units/result.json` records twelve
worker cases per mode, ten grant cases per mode, and all-target strict Clippy in
both modes. It proves retained execution, terminal worker fencing, the expired
ninth admission behind eight metadata tombstones, readonly versus mutator
control, and preservation of existing protocol/owner-control behavior. Large
unit executables are gzip-sealed under the compile lock and expanded only as a
single owned file in private container tmpfs. No test extracts a filesystem tree
or recursively removes fixtures.

The real dependency proof uses immutable executable SHA
`55ae8b3197d6e129d5ee8949604a2a1fe3aa376edb6d10c79aa2c7c3ccea4dca`:

- `/var/tmp/o3-server/page-execution-terminal-physical/result.json` records the
  distinct Node process, actual original waitFor, private Chromium and actual
  Control admission. The caller times out at 500 ms while the native wait and
  original RunContext remain entered. Human takeover does not end that readonly
  callback. A controlled DOM release completes the real native work and only
  then its completion notification. The production observe entry works under
  human control; an agent write and mutator observe entry are refused.
- `/var/tmp/o3-server/page-execution-isolated-runtime/page-pjndxssi/proof.json`
  repeats only that bounded physical case using the same immutable executable
  and the exact committed runtime at 3b8b866. The runtime source receipt is beside
  the proof. No scoped-gateway JavaScript or uncommitted cleanup change is needed
  by this execution consumer. Both runs acknowledge two individual session
  detaches and two native recursive-cleanup requests retained by quarantine,
  with zero executed recursive removals.

The executable was built in the isolated working tree while private gateway
source was also present but unselected. The selected worker/control bodies match
this checkpoint; the worker module's omitted NativeGateway public reexport is a
declaration-only difference. The accompanying proof records both source hashes.
No claim is made for gateway activation, outer MCP dispatch, all twelve tools,
eight simultaneous native timeout/terminate sequences, or final payload parity.
