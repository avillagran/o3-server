# Bounded server deltas from nightly4314

Reference: `83a3674ffd84ffb33de768210c806cb1ac53fd80`.

The default inactive-thread auto-settle threshold changes from three days to
seven. Explicit saved values, including three and null, retain their meaning.
Both baked server defaults and the active settings codec use seven. The closed
codec retains its original full-schema provenance; `defaultOverrides` records
this single newer source-backed default. Historical full-schema and AST corpora
remain unchanged. Their test adapter projects only the independently proven
missing-field default into the relevant outcomes, preserving all other assertions.
This does not certify the complete4314 schema or add cloud speech settings.

Git receives `LANGUAGE=C` before explicit caller environment is applied. It no
longer forces `LC_ALL=C`: inherited `LANG`, `LC_CTYPE` and `LC_ALL` are preserved
for hooks, and a caller may override `LANGUAGE`. The new override applies only to
the literal git command. Other command and installer environment behavior is
unchanged. The whole pinned native `VcsProcess.make` runs against an inert
process boundary in the source oracle; actual owned child fixtures check the
resulting environment in the private container.

Native Pi MCP injection now writes a complete extension in a unique staging
directory and renames it onto the destination, retrying rename three times.
O3 currently has no Pi MCP extension materializer or executable Pi provider
consumer; Pi remains explicitly disabled as unsupported. No unused installer or
placeholder success path is added. A future real Pi integration must port the
atomic publication semantics with owned staging and bounded empty-directory
cleanup rather than recursive scoped removal.

The generator verifies pinned source hashes, canonical Effect4.1.2 schema
identity, seven actual settings outcomes and24 whole VCS locale traces. Runtime
fixtures are private and retained. No live service, repository configuration or
provider installation is changed.

`python3 tools/apply-nightly-defaults4314 --fixture tests/fixtures/nightly-small-4314.json`
reproduces the two active default artifacts byte for byte from the retained
native observations. Regenerating the original full-schema pin alone is not a
claim of4314 schema support; this explicit default overlay must remain applied
until that complete schema is upgraded.
