# P1 reader inventory (2026-11-02)

Paths below are relative to the named sibling repository. This is the initial
implementation inventory, not a status promotion. Real-reader evidence must use
the pinned protocol dependency and an actual Connection. Independent stream
interleaving follows CONFORMANCE.md's stream reader contract; it does not excuse
missing or stale records.

| Row | Stated remaining gap | Production consumer | Planned action |
|---|---|---|---|
| assets.createUrl | MCP HTML, platform discovery, media adapters | lib-t3-rust `src/assets.rs:84` downloads grants; o3-code `src/write.rs:427` uses assets API | Trace resource builders; retain only consumed gaps; compare isolated assets. |
| orchestration.searchThread | Rich labels, legacy context, Markdown segmentation | No RPC consumer: o3-code `ui/qml/T3Panel.qml:193` searches loaded transcript; ot3-android `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/TimelineReader.kt:87` calls local transcriptMatches | No client-read gap in this RPC; retain tests, do not implement unused segmentation. |
| orchestration.searchThreads | Palette scoring and bounded fallback | o3-code `src/thread_search.rs:25` calls SEARCH_THREADS, `:39` preserves match order; ot3-android `core/src/main/kotlin/dev/avillagran/ot3android/core/home/MessageSearch.kt:11` | Compare ranking with reference; do not waive consumed ordering. |
| orchestration.subscribeCommandOutput | Claude live output, multiline VT | o3-code `src/command_output.rs:80`; ot3-android `core/src/main/kotlin/dev/avillagran/ot3android/core/tools/LiveCommandOutput.kt:14` applies commandOutput.apply | Compare real output through shared reducer; distinguish upstream completion-only delivery from missing server folding. |
| server.discoverSourceControl | API auth, GitCafe, desktop credentials | o3-code `src/write.rs:74` discovery for project creation; lib-t3-rust `src/api/server.rs:154` | Keep unsupported auth explicit; determine which descriptors clients consume. |
| vcs.refreshStatus | Other hosts and stale PR fallback/backoff | o3-code `src/git_controls.rs:22`, `src/branch_picker.rs:87` | Compare isolated host failures; PR metadata is consumed, not exempt. |
| thread.fork | Historical native resume-at | o3-code `src/write.rs:263` sends explicit run point; ot3-android `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt:684` | Verify historical context boundary; native opaque resume implementation alone is not a reader gap, lost history is. |
| thread.model-selection.set | Cross-provider context | No production command builder found beyond lib-t3-rust `src/api/orchestration.rs:36` allowlist; o3-code `src/write.rs:189` sends per-message selection instead | Test explicit idle command through real timeline reader; no claim that per-message switching is fixed. |
| thread.user-input.dismiss | Native delivery/recovery | lib-t3-rust `src/questions.rs:37`; ot3-android `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/MessageSender.kt:78` | Test actual native question dismissal, command builder and pending reader; isolate recovery uncertainty. |
| subscribeServerConfig | External/custom setup and browser service; other config gaps | lib-t3-rust `src/native_settings.rs:119`, `src/scoped_settings.rs:47` | Separate owner decisions (external CLI updates, browser service) from startup/custom/tier/normalization reader gaps. Owner decisions cannot excuse the latter. |
| server.updateSettings | Execution coverage, browser, Codex text generation | lib-t3-rust `src/native_settings.rs:172`, `src/scoped_settings.rs:47`; ot3-android `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt:954` | Separate owner decisions; retain tool-free Codex generation and any uncovered settings effects as non-owner gaps. |

No status is changed by this inventory. Live authentication and inference are
outside this run; deterministic fixtures must supply protocol evidence.

## Verified outcome and opinion

- `searchThread`: corrected the initial historical assumption. A full literal RPC
  search of the three source trees finds no consumer; both find UIs are local.
  Do not invent rich-segmentation work for this row. Independent audit should
  decide whether it belongs outside the used-method table; there is no actual
  lib RPC reader with which to meet the strict promotion prerequisite.
- `searchThreads`: **keep open**. `p1-reader-before.json` records an actual
  default-query counterexample, including match order and snippets rather than
  scalar shapes. Reference returns no message hits for matching titles; o3 does.
  Both apps consume these hits. Default search now excludes these title-only hits; the real lib reader
  regression and final `p1-reader-report.json` compare this correction. Full
  message ranking and bounded-fallback equivalence remain unproven. Explicit
  titlesOnly behavior is retained, without a reference-equivalence claim.
- `assets.createUrl`: attachment byte equivalence is established for the isolated
  HTML upload. **Promotion remains an independent audit question**, with broader
  media equivalence unproven here. Correction: no production caller sets
  renderIntent:mcp-app; only the optional lib API descriptors expose it. Desktop
  `src/documents.rs:10` fetches ordinary inline attachments and
  `ui/qml/T3Backend.qml:1202` supplies its own MCP bridge. MCP usage does not make
  server HTML injection a consumed asset requirement. Actual media consumers are
  desktop `src/videos.rs:28` and Android core `transport/NativeT3Transport.kt:70`.
- `subscribeCommandOutput`: the actual lib `parity::output` reducer now consumes
  real subscription frames and agrees on replacement/append/final text. Desktop
  `src/command_output.rs:80` assembles text directly, so server folding matters.
  **Keep open** pending reference multiline control/output evidence; prior live
  Claude evidence alone does not establish pinned 3991 streaming equivalence.
- `discoverSourceControl`: **keep open**; desktop `src/write.rs:75` reads auth
  status. GitCafe validation and desktop credential support remain unavailable;
  fixture classifications do not prove host acceptance. No credentials used.
- `vcs.refreshStatus`: **keep open**; PR lookup only implements GitHub/GitLab
  (`src/vcs.rs:433`, `:465`). Git controls/branch picker consume host status;
  other-host PR fields and last-known fallback/backoff lack reference evidence.
- `thread.fork`: **keep open**; callers select explicit historical runs, while
  historical handoff serializes text (`src/thread_commands.rs::handoff`). Native
  historical attachment/context continuity is not established by adapter-only
  latest-fork tests. No demand for opaque native implementation equality is made.
- `thread.model-selection.set`: explicit idle command now creates a fresh native
  identity and text handoff in both directions, verified by deterministic native
  peers, actual Connection and timeline reader. A negative control fails without
  the fix. **No promotion recommendation**: no production literal caller found,
  while desktop `src/write.rs:189` uses message.dispatch selection instead.
  Attachment/context and per-message cross-provider equivalence remain unproven.
- `thread.user-input.dismiss`: actual Claude native delivery now has positive
  questions-reader/builder coverage. **Keep open** until pinned reference and
  restart/delivery-unknown windows are established for native dismissal; the new
  test establishes live deny and disappearance, not all recovery schedules.
- `subscribeServerConfig` and `server.updateSettings`: **keep open**, separated
  into owner decisions (updating unknown-owner external CLIs; implementing the
  agent browser service) and non-owner evidence gaps (startup/custom discovery,
  generic option normalization/effects, tool-free Codex text generation). The
  latter are consumed by lib native/scoped settings readers, not exceptions.
  Existing positive settings/delta tests remain valid; this run does not certify
  the entire settings effect matrix.

These opinions are deliberately narrower than a claim that every listed gap has
been fixed or reproduced. Unproven items above need further implementation or
isolated evidence; none was converted to an accepted difference or a green row.

Concrete non-owner settings limit: `src/title.rs:67` rejects the Codex selection
in safe_selection; lib `src/native_settings.rs:56` exposes text-generation model
choice. This is separate from either owner decision. Concrete source-control
limit: `src/vcs.rs:961` reports GitCafe validation unavailable, while desktop
`src/write.rs:75` reads auth.status. Neither is an unread diagnostic-only field.

The consumer trees were searched with rg across Rust, Kotlin and QML, including
RPC literals and function spellings. A registry allowlist or test-only use was not
counted as a production caller. Native future behavior, opaque credentials and
platform adapters were not inferred from successful JSON decoding.


## Audit follow-up correction (2026-11-02)

The preceding inventory records earlier evidence, not current missing behavior.
Historical attachment continuity now works within documented bounds; native
latest-completed forks bypass fallback-only limits. Historical fresh sessions
also receive bounded tool-result context. Per-message provider selection has
Connection/native-peer coverage. PR fallback includes endpoint ports and search
retains matches beyond the former candidate window.

Remaining-gap claims must name a consumed field or behavior and its failing
reader. Unproven codecs, full VT emulation, live authentication, or opaque native
resume implementation equality alone are not consumed gaps. Existing row
statuses remain untouched pending independent audit. Other-host PR metadata,
broader search ranking, desktop credential storage and settings effect coverage
still need specific consumer/reference evidence. Codex generation remains
unavailable: settings saves work, while explicit generation gives an actionable
error instead of silently substituting another provider.
