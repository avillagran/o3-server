# Terminal provider environment oracle

The fixture executes the complete `resolveProviderInstanceTerminalEnvironment`
from nightly 4288 (`32a0531748c44b25072d7495ce0b9bd7e9450a87`). Its dependency
closure executes the real configuration-map derivation, environment merge, home
expansion, Codex home layout and Claude environment helpers. Codex, Claude,
provider-instance and terminal schemas come from the pinned contract sources and
Effect 4.1.2. Eleven driver metadata rows are extracted from the actual built-in
list, package exports and driver declarations; no provider launches execute.

Settings reads are inert services, including one typed read failure. HostProcess
uses `/fixture-home` and Linux/Darwin service values; the Path service uses actual
Node POSIX functions with resolution bound to `/fixture-cwd`. No filesystem home
preparation, private-store access, PTY or browser operation runs in this oracle.
Source hashes and the verified staging-manifest hash accompany every fixture.

The 315 helper cases include default instances, missing instances, all seven valid
Object-prototype method names, configured environments, unresolved references,
primary/shadow paths, invalid configuration and explicit-value precedence. The
Rust differential consumer compares the 307 admitted-ID cases whose settings
read succeeds. Seven raw-helper schema failures and the injected service failure
remain explicit observations outside that pure resolver boundary.

Sixteen additional cases run both actual `ProviderInstanceId` decoding and
`.make`. They differ: decoding trims an encoded `" codex "`, while `.make`
validates the already-decoded value and rejects that string. Another 34 cases
execute the actual terminal open/attach schemas, including omission, null, wrong
types, whitespace and the length bound. Those admission observations are separate
from resolver parity and require physical RPC acceptance before claiming the
server enforces the same boundary.

All Effect imports must resolve to one canonical module path shared by the
contracts. The generator resolves from the staged source package and verifies a
real provider-ID guard before executing cases. Mounting the same dependency tree
at two paths does not establish module identity. An earlier discarded run used
two Effect identities and incorrectly skipped valid provider configuration; it
is not acceptance evidence.

The physical provider-ID regression rejects twelve invalid encoded values and
opens/closes a real private PTY for a Unicode-whitespace-wrapped Codex ID in both
feature modes. That validates this field's admission before provider preparation;
it does not claim complete terminal DTO or native diagnostic-message parity.
