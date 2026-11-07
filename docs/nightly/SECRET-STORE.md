# Private secret-store and settings transaction leaf

The source boundary is T3 Code nightly `32a0531748c44b25072d7495ce0b9bd7e9450a87` with Effect 4.1.2. This is an isolated implementation leaf. It does not activate a settings writer, migrate authentication material, change signing or session identities, or advertise complete private-secret support.

## Native observations

`tools/secret-store-native.mjs` loads the complete native `ServerSecretStore` implementation. Its configuration service contains only a synthetic secrets directory. The filesystem and random services are injected; all observed file operations run against an in-memory map. The settings oracle loads complete original planning, redaction, echo, materialization, apply, and reverse-rollback methods. Secret-field annotations and secret-provider definitions come from their actual source registries. The source closure is checked against the pinned Git objects; the contracts closure must match byte for byte.

`tests/fixtures/secret-store-4288.json` contains:

- 27 store observations: missing and existing slots, replacement, exclusive publication, cached identity bytes, filesystem/random failures, concurrent-create winner recovery, disappearing winners, and a colliding temporary name.
- 670 settings plans and modern/legacy client projections, covering the 134 existing native schema-declared secret mapping inputs across retention, removal, empty values, echoed markers, and replacement.
- Eight whole native apply/rollback observations, including a write that mutates before reporting failure, rollback failure, and preservation of the primary error.
- 402 materialization observations covering missing files, stored UTF-8 values, BOM removal, and malformed UTF-8 replacement; 61 read-error observations preserve provider/environment metadata.

The Rust differential tests compare all 670 plans, both client projections, 402 materializations, 61 error projections, and eight transaction outcomes to these observations. The 27 lower-level store traces remain source evidence; the operating-system implementation is separately tested with actual private files rather than claimed byte-identical syscall traces.

## Rust boundaries

`server_secret_store.rs` exposes `SecretStore` (`get`, `set`, `remove`) plus `FileSecretStore::create` and `get_or_create_random`. Directory components and slots are opened relative to held descriptors with `O_NOFOLLOW`. Existing random-secret bytes are returned unchanged regardless of the newly requested length. Exclusive publication uses a hard link, and a concurrent loser rereads the winner.

`settings_secret_transaction.rs` exposes planning, reference echo, client redaction, materialization, apply, and explicit reverse rollback. A transaction snapshots each previous slot before mutation and records the attempted write before executing it. Failures roll back all recorded slots in reverse order. Rollback errors are retained separately from the primary error for a future consumer to log. A successful apply returns a rollback token so the settings-file writer can restore slots if its own later stage fails.

The caller must hold the settings write serialization boundary across plan, apply, materialization, settings-file publication, cache update, and rollback. This leaf has no independent settings-file writer, materialization cache, invalidation generation, publication event, or management-policy admission. Those existing consumer boundaries must be preserved when activation is proposed.

## Safety differences and limits

The native `set` method initially writes a temporary file before chmod, and native `create` unconditionally attempts temporary-path cleanup even when exclusive opening fails. This implementation deliberately creates temporary files exclusively with mode `0600` and only cleans a temporary entry whose device/inode still matches its owned file descriptor. It never cleans a pre-existing colliding temporary entry. The containing directory is secured to `0700`; slots are `0600`. Slot names are restricted to the ASCII characters emitted by the settings mapper, preventing traversal. Directory, symlink, FIFO, and other nonregular slot reads are refused. These are explicit safety restrictions rather than claims of identical behavior for arbitrary native service names or hostile filesystem traces.

No recursive deletion, recursive cleanup, physical mount, real profile, host credentials, authentication service, or actual vault is used by these tests or generators. Symlink fixtures remain within a private container temporary directory. The fixture directories are retained until container teardown. Only the exact owned slot or owned temporary file can be unlinked.

The native atomic settings writer is intentionally outside this oracle: its scoped temporary-directory cleanup uses recursive removal. Any future writer proof must use an in-memory filesystem or exact owned-file cleanup and must not execute that native cleanup physically.

Standalone integration tests include the new files directly, so no server module declaration or consumer activation is needed. Run only this audited target in rootless Podman:

```sh
tools/test-scheduled-modes --integration secret_store --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
tools/test-scheduled-modes --integration secret_store --default-features --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
```

Broader test execution remains subject to the repository's independent fixture-cleanup safety audit. JavaScript UTF-16 isolated surrogate inputs remain outside the Rust JSON/string boundary. Filesystem error representations are projected to typed store/settings operations rather than reproducing platform exception objects.
