# Structured composer context

Native version-one message context now travels separately from the user's stored
text. The provider receives T3's reference markers and one escaped `t3_context`
envelope containing only referenced records. The stored message and turn item
retain their original text and normalized context for display, retry and history.

The implementation targets official nightly 4262 (`c55db3e9`) and Effect 4.1.2.
The covered shared projection and contract files are unchanged in nightly 4275.
This is evidence for the context surface, not global nightly certification.

## Supported records and intake

All ten native kinds are covered: image, file, terminal, element, preview
annotation, review comment, mention, skill, thread and environment. Unknown kinds
carry bounded JSON payloads. Known malformed records are dropped by the native
forward-compatible decoder; an unknown payload cannot impersonate a known kind.
Duplicate context IDs after decoding reject the context before admission.

Labels, line ranges, optional versus nullable fields, nested records and serialized
size limits follow the pinned contracts. Projection preserves JavaScript number
spelling, integer-key ordering, non-index property insertion order, UTF-16 limits
and tag escaping. `serde_json` must retain insertion order for unknown payloads.
Shared-storage hashes canonicalize their own JSON independently.

An explicit context version selects this native path. Older unversioned O3 context
remains legacy additional context, including older incomplete records arrays.
It is not newly injected into native portable history. Unsupported explicit
versions are rejected rather than silently treated as legacy context.

## Provider and history paths

Both provider starts, direct steering, queued edits and promotion to steering use
the same projection. Launch preparation validates context before allocating
claims or preparing a workspace. Codex goal objectives are projected once;
materialization does not reinterpret literal references inside captured context.

Portable history and side-chat digests project native context as reference data.
Older turn items that omitted context recover it from their original user-message
row without rewriting either stored record. Original user text remains available
for display and editing.

## Evidence

`tools/generate-composer-context-fixtures.mjs` executes the complete pinned shared
modules and real contracts after type erasure and import relocation. It verifies
source hashes and Effect 4.1.2. The committed fixture contains 178 projections,
172 decoding cases and 12 compact size-boundary scenarios. It includes deterministic
mixed references, unknown payload ordering, repeated IDs, invalid references,
Unicode labels and nested records.

`tests/composer_context.rs` drives real RPC calls through both provider
adapters with private synthetic Claude/Codex peers. Their complete received
prompts match native oracle output. Tests also cover direct and queued steering,
normalized stored records, and rejection before launch/message admission. Provider
input, history hydration, orchestration readers and goal regressions cover the
additional integration paths. These tests perform no inference.

All oracle generation and test bodies execute in rootless Podman with read-only
source/binaries, private state and no external network. Host execution is limited
to compilation. See `composer-context-intake.json` for provenance and exact scope.

## Existing boundaries

The current RPC `Value`/`String` transport rejects isolated UTF-16 surrogate code
units before context intake. Valid Unicode, surrogate pairs and source truncation
boundaries are covered; this transport limitation is not certified away. The
native desktop terminal producer explicitly avoids a trailing high surrogate.

Provider-specific input limits and broader driver/session lifecycle keep their
own acceptance requirements. Native context schema acceptance does not promise
that every provider accepts every message size. The complete nightly inventory,
platform acceptance and managed policy remain separate work.
