# Managed settings intake

The native reference is `c55db3e9c72fdf584f4b54192fb39beb88667b59` (nightly 4262), using Effect 4.1.2. `managed-settings-intake.json` records the complete source closure and the comparison with nightly 4275. This intake covers the settings family, not global application parity.

`tools/generate-managed-settings-codec.mjs` exports the actual native Effect AST. The generated program covers the complete `ServerSettings`, `ServerSettingsPatch`, and persisted settings schemas. It includes encoded optional keys, defaults, nested structs and records, unions, all refinements, Duration values, and every schema transformation. Unknown AST tags, transformations, and refinement families abort generation. The Rust interpreter does not infer validation from the shapes of default values.

The same generator loads the pinned `managedSettings.ts` and `settingsSecrets.ts`, provider client schemas, and the real shared source-control definition registry. Its source-control secret helper extraction retains the exact pure implementation; it excludes the unrelated secret resolver and server driver imports. Credential field metadata comes from each native schema's `secretSettingsFields`, including environment variables and keyed host credentials.

Run the generator only in a rootless, network-isolated Podman container. Mount the checkout, contracts with Effect 4.1.2 dependencies, repository tools, and Node runtime read-only; mount only the output directory writable. Arguments are `CHECKOUT CONTRACTS OUTPUT_DIRECTORY`. The generator verifies every imported contracts source against the reference revision. Copy the codec program into `src/managed-settings-codec-4262.json` and the four fixture files into `tests/fixtures/managed-settings*-4262.json`. The generated `native-runtime` directory is temporary oracle material and is not a production dependency.

The fixture comparisons cover native decoded and re-encoded values for every schema node, whole-settings mutation and missing-key behavior, sparse persistence, legacy token streaming, policy admission reasons and ordering, defaults, nullable paths, credentials and references, project overrides, runtime ceilings, write refusals, preservation of stored locked values, legacy idle compaction, and atomic restoration of user choices stripped as defaults.

Rust represents an Effect Duration as its numeric millisecond value. The oracle normalizes native decoded Duration objects to that internal representation; encoded wire and persisted values remain byte-compatible JSON numbers. Missing and null stay distinct. Production execution does not invoke Node or the reference checkout.

Settings-family readiness remains controlled by the managed policy enforcement gate. The leaf's fixture tests alone do not establish that every physical read, registry launch, write, and sparse persistence path is wired correctly.
