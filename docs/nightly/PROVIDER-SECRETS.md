# Provider secret references

Registered Codex and Claude instances now resolve configured environment references through the server's shared secret resolver before creating a native provider scope. Their actual native configuration schemas are decoded first, including defaults, binary fallbacks, custom model validation, and diagnostic paths. Unknown drivers, disabled instances, invalid configuration, and policy refusals never trigger vault reads.

A registry keeps stored entries separate from resolved values. Initial reference reads run asynchronously, so configuration inspection can show the native `Reading secrets…` state without blocking startup. New instances share a prefetch batch. Failed or whitespace-empty values park the entire instance as unavailable with the native variable/field names and refresh instruction; no worker can inherit another credential instead. Literal variables and disabled-instance behavior follow the native mapper. Stale asynchronous results cannot replace a newer entry, and registry destruction cancels its outstanding reads.

`server.refreshProviders` honors `reloadSecrets` and optional `instanceId`. It invalidates only the enabled target instances' references. A working instance keeps its prior resolved values if a re-read fails. Successful re-reads preserve the existing scope when values are equal and retire it before replacing it when credentials change. Unrelated settings changes and ordinary provider refreshes do not retry parked failures. Changing a secret provider's account takes effect on explicit secret reload, matching the native registry lifecycle.

The mapping uses schema-declared reference slots, including keyed configuration fields. The active Codex and Claude schemas declare no configuration secret slots; an arbitrary `apiKey` property therefore cannot cause a vault read. The generic mapping tests also execute the native declared-field helper with a real settings-secret schema to cover direct and keyed slots. Decoded references are not decoded a second time: raw configuration slot lookup and reference spelling preserve the native distinction between schema admission and provider-specific reference validation.

Each State owns one Arc of the resolver, shared with its production provider registry. Resolution never changes settings files, public settings snapshots, message text, or the user's secret store. Source-control credentials, usage sources, device hosts, and reference answers to secret requests still need separate consumer integration; adding provider support does not advertise those capabilities.

The native source intake is recorded in `provider-secrets-intake.json`. Independent generation from nightly 4262/4275 and nightly 4288 produces the same settings/provider codec program and fixture results. The new binary-path fallback and native diagnostic expectations are exported from the actual Effect AST. Tests compare all schema nodes, complete registered-provider configuration errors, the native secret mapper, and registry cache transitions. Private synthetic op processes and both actual native provider transports verify credential delivery, failure parking, rotation, shared cache lifetime, and absence of resolved values in persisted/public settings. Fixture executables are copied into private directories; no fixture symlinks or host authentication mounts are used.

Run focused checks through `tools/test-scheduled-modes --unit-filter provider_secrets::tests` and `--integration provider_secrets`, adding `--default-features` for the default build. Serialize compilation with the repository's build flock. Local test bodies always run in rootless Podman. Verified disposable GitHub Docker job containers use the shared test sandbox guard; direct host execution of test binaries is not supported.

Physical private-environment receipts correlate the exact Codex `turn/start` or
Claude `user` request by driver, independent fixture instance, process ID and
unique message marker. Completion barriers match `run.userMessageId`. A genuine
neighbor request deliberately becomes the latest receipt before the target is
asserted, so cross-instance discovery order cannot select the wrong process.
Launch receipts remain separate for neighbor restart-count assertions. Claude
may send an empty native session ID before its first init event; its app-thread
environment identifies that initial request. Public settings, private-slot
rollback and server identity assertions remain unchanged.
