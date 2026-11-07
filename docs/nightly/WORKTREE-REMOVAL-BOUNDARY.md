# Destructive worktree operation boundary

Storage policy collection, staged-launch failure cleanup, handoff rollback and
legacy workspace preparation can remove an owned Git worktree. Those operations
share an explicit per-State execution dependency. With no replacement configured,
each production caller uses its original real Git runner, argument vector,
ownership checks, locking and timeout. The same boundary covers owned branch-ref
cleanup, which otherwise conflicts with deliberately retained test registrations.
There is no environment variable or compile-time test bypass.

Private integration fixtures explicitly install a retaining simulator. It records
the repository, checkout, selected single-force flag and literal argv. Before any
change, it verifies that paths remain within the declared private fixture, rejects
symlink components, reads the Git common directory and verifies that the exact
worktree registration points back to the expected checkout. A locked registration
is refused, matching the covered single-force behavior.

The simulator moves the exact checkout and registration directory into a private
quarantine. Owned loose branch-ref and reflog files are retained by rename too.
It never invokes `git worktree remove`, `git worktree prune`, `git clean`, recursive
filesystem removal or an archive extractor. Moving the fixture registration out
of Git's active namespace permits real retry/preparation code to run while keeping
all physical trees and metadata available for assertions and inspection.

Tests assert retained checkout contents, registration pointers and recorded
ownership/argv, alongside storage-policy outcomes, protected worktrees, confirmed
process exit, cleanup retry and successful subsequent preparation. The simulator
models only the explicitly covered private fixtures; it is not a replacement Git
implementation or evidence that a real production checkout was physically wiped.
All execution uses the existing read-only-source, private-HOME/tmp rootless Podman
harness, or its explicitly verified CI container environment.
