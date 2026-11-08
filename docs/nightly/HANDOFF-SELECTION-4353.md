# Nightly 4353 handoff selection

The selector now skips a historical message whose UTF-16 text length already
exceeds the remaining byte budget, before rendering or serializing that message
to calculate its exact cost. This ports the early rejection in upstream commit
[`8a84e942`](https://github.com/pingdotgg/t3code/commit/8a84e9426d26bb50de24a8d36c262a851a3befd2).

Rust stops counting after `remaining + 1` UTF-16 units. Even a very large rejected
message therefore avoids a complete scan and the temporary rendered/JSON copies.
The exact cost calculation, intact-message priority, chronological output and
omission counters remain the existing selection algorithm.

## Validation

The private comparison executed the complete original `handoffBudget.ts` from
Nightly 4353 (`3cd97a1ee230437b1bfd8441dffe65eac55be1e5`), with the actual Effect
4.1.2 Config module. It compiled the exact closed scalar/render/response/cost/select
source from the changed Rust file with `rustc -Dwarnings` and compared all 63
results, including empty histories, small messages, oversized ASCII and emoji,
large latest messages, mixed Unicode and 60-item histories at nine budgets.
All 63 comparisons passed.

The comparison ran in rootless Podman with no network, private HOME/tmpfs, no
host credentials or devices, and read-only source, dependency and toolchain
mounts. Cargo did not run. Its probe binary existed only inside the container.
The same owned CLI exited successfully and the container name was confirmed
absent. This 1.52-second run finished before a CID was retained; it does not
claim a separate retained-CID observation.

Evidence is retained at
`/var/tmp/o3-server/handoff-selection-4353-proof-lj7fl5jb/`. The result and input
hashes are recorded in [the scoped receipt](handoff-selection-4353-proof.json).
This proves the scalar selection component. It does not establish whole-server
build, full handoff admission, generic lone-surrogate handling or whole-Nightly
parity.
