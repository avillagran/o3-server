# Device worker stack bounds

The device consumer suite exercises the complete protocol2 WebSocket dispatcher,
registered SSH owner, session lifecycle and authenticated media HTTP path on a
Tokio runtime whose workers have the production 512 KiB stack budget. The worker
measures its actual pthread allocation as 524,288 bytes. Profile preparation runs
before the consumer task, matching production startup. The test uses a private
SSH-shaped TCP CLI and hub; it never runs SSH, an SDK or a device tool.

## Observed failures and bounded fix

Private debug unit runs reported Rust stack overflow and exited with code 139.
Matching coredump metadata recorded SIGSEGV, including failures in ordinary test
threads and the explicit 512 KiB worker test. The latter failed at the first
`disabled list` consumer stage. The same flow passed on ordinary 2 MiB test
threads, so ordinary test success alone did not establish production stack margin.

The initial matching processes had approximately 45 MiB and 35 MiB resident
memory against a 4 GiB private-container budget; no matching OOM journal entries
were found in the crash interval. The recorded process stack soft limit was
8 MiB. That process limit is distinct from Rust/Tokio worker allocations. Coredump
stacks were unsymbolized; no raw core was extracted or deleted.

Measured debug future layouts before and after the fix:

| Boundary | Before | After |
| --- | ---: | ---: |
| Device budget dispatcher | 34,576 bytes | 2,048 bytes |
| Inner device dispatcher | 34,312 bytes | 6,704 bytes |
| Public device call/state handle | 16 bytes | 16 bytes |
| General WebSocket RPC future | 4,296 bytes | Unchanged |
| General unary dispatcher | 5,304 bytes | Unchanged |

Two inline 16 KiB SSH output buffers inflated every awaiting dispatcher. Debug
assembly reserved approximately 239,864 bytes in the inner dispatcher poll
function and 182,120 bytes in the budget dispatcher poll function before nested
work. Boxing only the outer future did not remove these poll-stack reservations.
The buffers now use bounded heap allocations, and the inner device dispatch is
boxed independently. Local/SSH owner dispatch also uses bounded boxed futures.
The public limits, stream budgets and worker stack size remain unchanged.

These measurements and the successful worker-budget regression support excessive
debug stack pressure as the mechanism. They do not reconstruct an exact historical
call chain from a symbolized core. The acceptance test proves the covered debug
consumer flow under the production budget, not every hardware path, compiler
version or release optimization layout.

## Reproduce the acceptance check

Run through the repository's private-container harness:

```sh
tools/test-scheduled-modes --compile-lock /var/tmp/o3-build/o3-parity-compile.lock --unit-filter ssh_rpc_media_consumers_fit_production_512kib_worker_stack
```

The test runs on Linux, where the pthread allocation is inspected directly.
Diagnostic layout experiments do not poll futures or execute SSH/SDK commands;
raw snapshots and assembly evidence stay in private build artifacts. No desktop
notification settings or system crash-capture configuration are changed.
