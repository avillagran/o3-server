# Safe pinned device tool acquisition

Source pin: T3 Code nightly 4288,
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, Effect 4.1.2.
The source-derived toolchain corpus executes the native module with entirely
in-memory filesystem and process services. Its 24 outcomes cover hub and agent
pins, paths, 10-minute npm budget, completed-version reuse, failed commands and
missing entries. Native recursive cleanup is recorded as data and never executed.

The Linux acquisition consumer uses pinned `expo-device-hub@0.19.1` and
`agent-device@0.21.31`. Each transaction holds a persistent nonblocking acquisition
lock in an owned directory that is not writable by other users. It creates a
unique mode-0700 staging directory, runs argv-only npm with lifecycle scripts and
bin links disabled, and supplies empty user/global configuration and a private
cache. The existing process runner clears ambient runtime injection and bounds
process time and output. Errors omit raw installer output and credentials.

A successful exit does not publish a tool. Package name/version, entry and the
complete regular-file inventory must pass verification before the manifest is
sealed. Linux `renameat2(RENAME_NOREPLACE)` publishes atomically without replacing
any existing path. Existing incomplete or foreign trees and links are refused.
Every transaction retains its staging/cache artifacts under a unique quarantine
name; cancellation and failures do not recursively remove or follow trees.
Quarantine retention is intentional and needs explicit future maintenance.

The inventory records content integrity after acquisition. It does not establish
independent upstream provenance for package contents. npm uses its normal registry
transport and integrity validation; no mutable dist-tag or package lifecycle
script is accepted. The native mutable `npm@11` fallback and destructive installer
cleanup are deliberately unsupported. Non-Linux atomic publication is not claimed.

Manual selected-tool updates install and perform read-only inspection. They do not
enable device or agent permissions, start helpers, replace a running owner or
clear sessions. Failed updates preserve existing runtimes and can be retried.
Enabled local readiness acquires a missing hub only when local platform discovery
is available; disabled support and read-only inspection never acquire packages.
An already registered local owner keeps its existing verified tree.

Validation uses private synthetic packages and executables only. A loopback HTTP
server supplies a tiny test-generated tar containing two regular files; the fake
installer unpacks only those validated relative entries in its private directory.
No real registry package, SDK, emulator, remote host or root filesystem is fetched
or extracted. Link fixtures target sibling private directories. Tests retain all
artifacts without deleting trees. A synthetic Node HTTP hub exercises actual local
acquisition, manifest verification, owned startup/readiness and shutdown; its SDK
path is intentionally absent and no SDK executable runs.
