# Owned cleanup and explicit profile removal

Temporary and failed-work cleanup must not recursively remove directory trees.
Private Claude MCP configuration releases only the exact owned `config.json` file
and its now-empty, identity-checked directory. Unknown files remain untouched.
Title-generation workspaces, ephemeral browser profiles, failed project creation
and failed clone staging are retained by unique quarantine renames. Shadow-home
local directory replacement retains the previous directory before creating its
shared-state link. No cleanup walks links or recursively removes these trees.

Cloning uses a fresh uniquely owned sibling staging directory and publishes only
to an absent destination with atomic no-replace rename. Failed staging is retained;
existing destination paths, including empty directories, are refused explicitly
and never moved or deleted. This deliberately tightens the older clone behavior
that accepted an existing empty destination and could recursively remove it after
failure. Operators can choose a new destination and inspect retained work.

Explicit browser-profile deletion remains a production operation. It first waits
for all matching browser processes to acknowledge exit. A shared bounded removal
plan enumerates entries without traversing symbolic links and orders files before
empty directories. The default executor checks recorded device/inode identities,
opens every parent component with `O_NOFOLLOW`, inspects final entries without
following links, and uses only descriptor-relative `unlinkat` calls. Unexpected
replacement or extra directory contents cause a failure rather than traversal.

Tests inject a recording executor through the same preview manager API used by
the production browser lifecycle. They assert the complete removal plan and
confirmed child exit while retaining the entire physical fixture profile. The
shared plan is also applied to an in-memory filesystem. No test physically wipes
a profile tree; there is no environment or compile-time bypass that silently
changes production deletion behavior. Exact owned-file unlink tests retain
unknown files and sibling-only symlink targets in their private temporary root.

Quarantine retention is intentional. It preserves evidence and unknown contents;
no automatic pruning policy is implemented here. Retained directories consume
space until an operator explicitly reviews and removes them.
