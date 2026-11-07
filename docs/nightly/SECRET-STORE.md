# Private secret-store and settings transaction leaf

The source boundary is T3 Code nightly `32a0531748c44b25072d7495ce0b9bd7e9450a87` with Effect 4.1.2. The core is independently tested. Active consumers cover `sourceControlHosts`, `usageLimitSources`, and sensitive environment literals on the supported `codex` and `claudeAgent` instances. Other provider drivers and device private writes remain gated. This does not migrate authentication material, change signing or session identities, or advertise complete private-secret support.

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

The caller must hold the settings write serialization boundary across plan, apply, materialization, settings-file publication, cache update, and rollback. The selected consumer uses the existing settings lock, policy admission, atomic JSON publication and notification. It rolls back attempted slot changes if verification or settings-file publication fails, and invalidates the raw settings cache only after publication. Forge and quota readers take a settings-plus-slots snapshot under the same lock. A quota credential generation forces opted-in subscriptions to refresh when private bytes change while the public JSON stays identical. There is no new materialization cache. Other credential families and full client reference capability negotiation remain outside this activation. The `Settings` managed-policy startup family remains gated; this change does not enable it. Its existing internal admission/sparse-persistence unit regression is checked independently.

## Safety differences and limits

The native `set` method initially writes a temporary file before chmod, and native `create` unconditionally attempts temporary-path cleanup even when exclusive opening fails. This implementation deliberately creates temporary files exclusively with mode `0600` and only cleans a temporary entry whose device/inode still matches its owned file descriptor. It never cleans a pre-existing colliding temporary entry. The containing directory is secured to `0700`; slots are `0600`. Slot names are restricted to the ASCII characters emitted by the settings mapper, preventing traversal. Directory, symlink, FIFO, and other nonregular slot reads are refused. These are explicit safety restrictions rather than claims of identical behavior for arbitrary native service names or hostile filesystem traces.

No recursive deletion, recursive cleanup, physical mount, real profile, host credentials, authentication service, or actual vault is used by these tests or generators. Symlink fixtures remain within a private container temporary directory. The fixture directories are retained until container teardown. Only the exact owned slot or owned temporary file can be unlinked.

The native atomic settings writer is intentionally outside this oracle: its scoped temporary-directory cleanup uses recursive removal. Any future writer proof must use an in-memory filesystem or exact owned-file cleanup and must not execute that native cleanup physically.

The standalone core integration includes the new files directly. A separate real-RPC integration covers save, echo, replace, clear, reopen, reference-only legacy projections, identity preservation, validation before mutation and preservation of the managed-settings startup gate, rollback on publication/materialization failure, and actual synthetic HTTP refresh with identical public settings. Run only these audited targets in rootless Podman:

```sh
tools/test-scheduled-modes --integration secret_store --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
tools/test-scheduled-modes --integration secret_store --default-features --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
tools/test-scheduled-modes --integration settings_private --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
tools/test-scheduled-modes --integration settings_private --default-features --compile-lock /var/tmp/o3-build/o3-parity-compile.lock
```

Broader test execution remains subject to the repository's independent fixture-cleanup safety audit. JavaScript UTF-16 isolated surrogate inputs remain outside the Rust JSON/string boundary. Filesystem error representations are projected to typed store/settings operations rather than reproducing platform exception objects.

## Supported native provider environment consumer

The pinned native descriptors declare no secret configuration fields for Codex or Claude. This consumer therefore handles their schema-declared sensitive environment variables; it does not infer credential names inside driver config. Supported literal values are written to the native `provider-env-<instance>-<name>.bin` slots, with empty public values and `valueRedacted: true` in settings. Existing literal inline values migrate on the next admitted settings write. Echoes retain the saved value, and explicit replacement, clearing, or removal uses the same transactional rollback path as source credentials.

`ConfiguredProviders::with_settings_lock` shares the state's settings lock. The production executable wires it explicitly. A registry that was not given this lock refuses private-marker preparation rather than reading an incoherent snapshot or falling back to another account. Provider-pointer locks are released before inspection; the writer captures the pointer before acquiring the settings lock, preventing inverted lock ordering.

The registry reads the raw definition and the private environment while holding the shared snapshot lock, then releases it before any asynchronous external-reference resolution. A read-only secret-directory opener neither creates directories nor changes their modes and refuses symlink components. Missing slots become the native empty literal; unreadable or nonregular entries refuse preparation. External reference success/failure caching and explicit refresh semantics retain their existing separate boundary.

Stored definitions remain the identity used by the instance cache. A separate in-memory fingerprint of the materialized input invalidates preparation when private bytes change despite unchanged public JSON. Existing resolution tickets discard stale asynchronous completions. Resolved environment values remain in private runtime state; they are not copied into stored definitions or public configuration. Unsupported-driver transitions retain their opaque private state instead of treating the excluded entry as a removed supported instance.

Focused real native-peer tests run both Codex and Claude through literal save, launch, echo, failed publication, rollback, and replacement. A neighboring instance checks scope isolation. Additional tests distinguish missing/unreadable private slots, refuse hydration without the shared lock, verify reader/writer lock ordering, and preserve signing/session identity. The native descriptor fixture checks that configuration secret annotations remain empty. Fixtures use copied synthetic executables and private provider homes; they never mount real authentication state or invoke a real vault or inference service.
