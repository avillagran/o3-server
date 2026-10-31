# Real-client E2E — partial, 2026-10-31

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
