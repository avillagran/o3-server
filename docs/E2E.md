# Real-client E2E

## Base operativa, 2026-10-31 (binario release de `main`, perfil aislado)

| Flujo | Cliente | Resultado |
|---|---|---|
| Turno real de Claude (`claudeAgent`, haiku), streaming, checkpoint, completado | WebSocket real | PASS |
| Turno real de Codex, proceso bajo demanda, cierre tras 60 s inactivo | WebSocket real | PASS |
| Descubrimiento por perfil y `t3` (symlink), `snapshot` en `ready` | `omarchy-t3-client` real | PASS |
| `send` con respuesta real de Claude | `omarchy-t3-client watch` real | PASS |
| Visor: `bounded`, `subscribeThread`, proyección en vivo | `omarchy-t3-client watch` real | PASS |
| Interfaz QML | O3 desktop | sin probar |
| Android | O3 Android | sin probar, pospuesto hasta tener la versión operativa |
| OmaBridge status/read/identity/peers/publish/pair | `omarchy-t3-client` real | PASS aislado; detalles abajo |

Memoria de `o3-server`: reposo 29,7-32 MB sin hijos; pico 38,6 MB con turnos de Claude y Codex; 37,9 MB con el
desktop conectado. Los procesos de proveedor son aparte (Claude CLI ~240 MB, Codex ~390 MB) y solo existen en uso.

Nota: `scrollback_is_bounded_per_terminal` falló una vez dentro de la suite completa bajo carga y pasó 3/3 en solitario.

## Corrida parcial anterior

This run did not complete the requested end-to-end validation. No compatibility
status is promoted on this evidence.

- Built the unmodified `omarchy-t3-client` (0.2.24) in a disk-backed isolated
  target. Started the previously built integrated o3-server at commit
  `c269ce5d2a207ad2d7f453b38ade02519d9301c2` on an ephemeral loopback port,
  with isolated HOME, XDG paths, T3CODE_HOME, bridge state and a private `t3`
  symlink. Desktop `snapshot` returned local environment `ready`, empty projects
  and threads. This verifies initial discovery, not a thread or inference.
- Desktop `omabridge-status` returned `available: false`, `identity: false` and
  no peers with an isolated XDG data directory, despite embedded bridge startup.
  Desktop expects its separate OmaBridge binary/state; embedded desktop flows
  were not validated. Do not treat this result as proof that all bridge flows fail.
- The server's Codex probe reported `Codex stdout closed`: the PATH selected a
  mise wrapper whose configuration is untrusted with the isolated HOME. A direct
  `codex login status` against a copied auth file reported logged in, but no turn
  was run and authenticated inference remains unverified.
- Android: no emulator was connected at discovery. SDK images and KVM exist; no
  separate emulator was started in this partial run. Pairing, message and
  screenshot are unverified.
- Claude turn, approvals, stream, terminal, attachments, git status, settings,
  desktop thread view, Android and VmHWM during inference remain untested.

All test profile and build paths were under `/var/tmp/o3-build/e2e`; no installed
service or real T3 profile was intentionally accessed. Credential copies were
used only in the isolated profile and removed after the test.

## Embedded desktop bridge checkpoint

`tools/desktop-bridge-e2e` passes against the release host and rebuilt real desktop
`/var/tmp/o3-build/e2e/desktop-target/debug/omarchy-t3-client`. The desktop changes
are committed locally on `feat/o3-server`. Build commands used `nice cargo -j 4`,
`CARGO_TARGET_DIR=/var/tmp/o3-build/main`, `TMPDIR=/var/tmp/o3-build/tmp`; the desktop
executable was then copied to the specified E2E location.

Reproduce with `O3_BIN=/var/tmp/o3-build/main/release/o3-server
O3_DESKTOP_BIN=/var/tmp/o3-build/e2e/desktop-target/debug/omarchy-t3-client
nice tools/desktop-bridge-e2e` (environment assignments and command on one line).
The runner isolates HOME/XDG/T3CODE_HOME and a private PATH `t3` symlink, starts
two embedded hosts, and stops both in finally blocks. No credentials imported.

Verified: available/identity true, empty peers, read with embedded backend,
unpublish/publish, invite creation, guest desktop pair, host desktop approval,
guest retry to approved, and paired peer listing. The host facade also issues a
T3 pairing through the guest's control socket and starts a QUIC forward that serves
the real environment descriptor. Existing bridge integration tests independently
exchange the pairing with the real protocol client. UI rendering and persistent
desktop sharing units remain untested; no systemd units or services were touched.

Final feature-enabled suite: 101 passed, 3 ignored (live/reference opt-ins), zero
failures. Strict all-target clippy, fmt and diff checks pass. An earlier full run
hit the pre-existing `scrollback_is_bounded_per_terminal` timing assumption; the
terminal suite retry and final full suite pass. This does not certify that flake fixed.

Final `tools/bench-rss` release workload: VmHWM 39,404 KiB (40.35 MB), below 50 MB;
20 threads, 1,000 deferred messages, 3 subscribers, 1,000 settings calls. Bridge is
active, providers registered/probed, inference excluded. See rss-measurement.json.

The final desktop E2E also verifies that an explicit standalone route state directory overrides embedded auto-discovery. Desktop OmaBridge-filtered tests: 20 pass.

## Native input / atomic launch checkpoint (2026-11-01)

New deterministic real-client coverage in `tests/provider_input.rs`:

- Actual upload -> Claude base64 image / Codex localImage, file path readable by
  native tool, serialized composer context; native peer asserts the payload.
- Injected SQL failure after thread/message staging rolls back records/events/
  receipts; same launch retry produces one run and retains initial context.
- Claude AskUserQuestion projects through `t3_protocol::questions::pending`, then
  `questions::command` answers without a permission decision; native peer checks
  question-text keys and multi-answer joining.
- Pinned Claude catalog and native slash/skill listings in getConfig; effort and
  fastMode argv checked by the peer; shell user/run timestamps checked over HTTP.

Two short authenticated turns completed (Claude haiku and Codex gpt-6.1-sol), each
returning `O3_OK` through the real Connection. Reproduce with `tools/live-chat-e2e`:
it builds with nice/-j4, uses disk-backed temporary HOME/T3/provider directories,
copies only auth files, and deletes the isolated credentials afterward. This is
not live proof of attachments, plan workflow, persistent approval or recovery.

Final deterministic suite: 105 passed, 4 opt-ins ignored with bridge; 104 passed,
4 ignored without default features. Strict all-target Clippy, fmt and diff checks
pass. The isolated live runner passed both real turns and now explicitly shuts
both native drivers down before removing its profile (an initial successful-turn
run hit a cleanup race). The real desktop backend bridge E2E passes all reported
status/read/identity/peers/publication/pairing/QUIC checks. QML and Android UI remain
unverified; no emulators, services, real T3 profile or systemd units were used.

Final release `tools/bench-rss`: HWM 42,052 KiB = 43.06 MB, below the decimal
50 MB budget. Bridge active; 20 threads, 1,000 deferred messages, 3 subscribers,
1,000 unary calls; provider inference excluded. Exact binary digest and /proc
samples are in `rss-measurement.json`.

## Scoped approvals and plan follow-up checkpoint (2026-11-01)

Deterministic real-client suite: 110 passed, 4 ignored with bridge; 109 passed,
4 ignored without default features. Strict all-target Clippy, format and diff
checks pass. Native peer coverage now includes both proposed-plan capture and
implementation using the real plan helper, source-plan completion, observed
Claude quota windows, explicit deterministic title generation, and persistent
exact-operation approvals with database reopen plus a changed-command refusal.
Recovery closes streaming messages and orphan nodes/items; linked-worktree
rollback preserves root files and real worktree index. Settlement safety tests
cover queued work, approvals, subagents, uncertain controls, pins and opt-outs.

`tools/live-chat-e2e` passed fresh short authenticated Claude and Codex turns.
No live approval/plan inference or Android/QML UI validation is claimed. No push,
service/systemd changes, real T3 profile access, emulators or QEMU.

Fresh release validation: `tools/desktop-bridge-e2e` passes status, identity, read,
publication/unpublication, pairing, approved peers, T3 pairing and QUIC forward
against the existing real desktop executable and the newly built release server.
`tools/bench-rss` HWM is 44,568 KiB = 45.64 MB (decimal), below 50 MB; bridge active,
20 threads/1,000 deferred messages/3 subscribers/1,000 calls, inference excluded.
The exact binary hash and samples replace `rss-measurement.json`. P0 RPCs remain
0 done/5 partial; P1 18 done/13 partial, because consumed behavior still differs.
