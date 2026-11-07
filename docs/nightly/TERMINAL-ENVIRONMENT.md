# Provider terminal environment

Reference: nightly 4288, `32a0531748c44b25072d7495ce0b9bd7e9450a87`,
`apps/server/src/terminal/Manager.ts` and provider-core instance environment,
Codex home layout and Claude home helpers.

A provider terminal now reads materialized private settings before launch or
restart. Provider instance variables override the explicit terminal environment,
matching the native merge order. Unresolved secret references are not interpreted
as strings. Only the two provider home variables expand a home prefix; valid
Codex/Claude config home overrides use their native path/trim order. Missing
instances fail before PTY creation. An empty resolved provider map stays distinct
from an omitted environment.

Private PTY integration verifies precedence, sensitive inline materialization,
rotation on restart, missing-instance rejection and absence of private environment
values from lifecycle metadata. All 13 terminal tests pass in each feature mode;
shared all-target strict checks pass in both modes. Printed child output remains
terminal output and can enter history, as expected.

A whole-helper native oracle and prototype/path edge audit are still in progress;
these targeted results are not a claim of complete server parity. The previous
full suite stopped because the terminal fixture expected the opposite precedence
and waited indefinitely for a secret that had not been materialized. The fixture
now accumulates snapshot/live output with a bounded deadline.
