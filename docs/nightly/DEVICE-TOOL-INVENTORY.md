# Read-only device tool inventory

Source pin: T3 Code nightly 4288,
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, Effect 4.1.2.
The complete native `DeviceToolchain.deviceToolVersions` function runs against an
in-memory filesystem in the reference generator. Its 295 outcomes cover version
names, sentinel trim/equality, entry presence, lexical sorting, missing directories,
read failures and nullable running versions.

The physical reader enumerates only real version directories. A historical version
is reported installed only after its complete O3 manifest, package name/version,
entry and file inventory pass verification. Historical inspection never selects
an executable: process startup still requires the compiled fixed-pin `inspect`.
Native sentinel-only or modified trees remain untouched and are qualified through
`toolInspectionError`, rather than silently presented as absent or usable. Unsafe
or unreadable paths retain the last verified inventory with an inspection error.

Local host summaries expose the native `tools.hub` and `tools.agent` DTOs. Manifest
verification runs on a blocking worker. Hub `runningVersion` comes only from the
current owned process after inspection; agent running version remains null because
no agent runtime owner has been implemented. Inspection does not install, seal,
rename or remove files, start helpers, stop sessions or change feature capabilities.

## Device list action authorization

The generator also executes the pinned `RpcAuthorization.requiredScopeForDeviceList`
function and the actual `ws.ts` device-list handler expression with inert service
values. Actual `DeviceListInput` decoding is included. The 23 rows cover mutation
precedence, ignored valid retry fields during inspection, invalid types and Unicode
host-ID trim/length behavior.

Source anchors are `apps/server/src/auth/RpcAuthorization.ts:203` for the base
scope, `RpcAuthorization.ts:296` for the extra mutation scope, and
`apps/server/src/ws.ts:3225` for inspection/update/retry precedence.

The base RPC scope remains `orchestration:read`. An effective tool update or host
retry additionally requires `orchestration:operate`. An update takes precedence
over `inspectOnly`; without an update, `inspectOnly` takes precedence over retry.
Authorization and dispatch share one validated classifier. Ordinary enabled listing
keeps the existing lazy readiness explicitly enabled by the owner's device setting;
a read-only payload cannot force an update or retry action.

The HTTP WebSocket upgrade passes the authenticated session to this same RPC guard.
The separate device-media HTTP transport already requires operate permission. MCP
has no generic device-list/update dispatch route. The security regression drives
real HTTP upgrades and protocol2 frames: readers can inspect, mutations are refused
before creating installation state, and a read-plus-operate token can update the
synthetic pinned tool. It performs no live package, SSH, SDK or device operation.

Source/physical tests retain all fixture trees. Link targets stay in their private
temporary workspace. No real installed package, SDK, emulator or hardware is claimed
by this inventory proof.
