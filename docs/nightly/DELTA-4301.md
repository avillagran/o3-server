# Nightly 4301 delta audit

Reference: `v0.1.8-nightly.20261107.4301`, commit
`4cb6945dc60ed03f567dc5daafd083e52308e82b`, published 2026-11-07 03:21:37 UTC.
Previous reference: 4288 (`32a0531748c44b25072d7495ce0b9bd7e9450a87`).
This audit records new work; it does not upgrade existing 4288 evidence to 4301.

| Upstream change | Relevant behavior | Current status |
| --- | --- | --- |
| `240446e2d9` | Persist usage-limit continuation source on every run creation path; rearm only tagged unsent auto-resumes after restart; cancel/hide their old prompt; retain legacy untagged cancellation | Source audit in progress; existing local recovery is not evidence of equivalence |
| `d944e2a396` | Strip only an Electron token from per-request User-Agent headers for the two HTTPS Teams hosts | Client implementation and native callback/Qt transport proof in progress |
| `08a3909406` | Sidebar age uses the latest valid user/requested/started/completed timestamp, then updatedAt | Client consumer audit pending; reply sorting must not be conflated with age labels |
| `d5fc29fe5c` | Relay-connected microphone destinations authorize speech requests; browser/environment speech lifecycle changes | Source audit pending |
| `07508ffd2f` | Popover pointer interaction over desktop header | Qt consumer applicability audit pending |
| `b19c9f038a` | Installable web manifest/icon resources | Distribution-specific applicability audit pending |
| Mobile UI fixes | Settled status, composer font, native diff section reset and filter menu behavior | Separate mobile consumer changes; no Android/device parity claim |
| Release/build changes | CLI archive construction/notarization, artifact acquisition and brand assets | Packaging audit pending; no release or installation performed |

No upstream pull request, tag, release, deployment or service change is implied.
