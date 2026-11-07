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

## Full-suite pthread cache isolation

A later full minimal suite passed 339 cases, but this test rejected an
actual worker allocation of 2,097,152 bytes, despite requesting 524,288. This was
an assertion failure, not another observed stack overflow. The earlier stack
pressure fix and its production budget remain unchanged.

GNU glibc 2.41's
[`get_cached_stack`](https://github.com/bminor/glibc/blob/glibc-2.41/nptl/allocatestack.c#L52-L135)
selects an available cached stack at least as large as the request and rejects
it only above four times the requested allocation. Thus a completed 2 MiB thread
can supply a later 512 KiB request. The
[GNU pthread tunable documentation](https://sourceware.org/glibc/manual/latest/html_node/POSIX-Thread-Tunables.html)
describes `glibc.pthread.stack_cache_size`; setting it to zero prevents retaining
unused stacks. Tunables must be supplied when the child process starts.

A separate pthread-only executable reproduced this in the same private glibc
2.41 container, without invoking the failing device test:

| Sequence | Default cache | Cache disabled at exec |
| --- | ---: | ---: |
| Request 2 MiB, join | 2,097,152 | 2,097,152 |
| Then request 512 KiB, join | 2,097,152 | 524,288 |

The acceptance test now executes only itself in a fresh child with the cache
disabled, using the bounded process runner and a 90-second deadline. The child
rechecks container isolation, creates the original 512 KiB Tokio runtime, measures
the actual pthread stack, and runs the unchanged protocol2 SSH/media consumer
flow. Its parent also requires the measured 524,288-byte diagnostic, preventing a
zero-test child invocation from passing. The environment change applies only to
that child; production workers and other tests retain their configuration.
No assertion was relaxed, no worker budget increased, and no runtime path was
skipped. Reproduction artifacts remain under the private build cache.

## ADB settings readers and common pipe buffers

A subsequent private full run reached `tests/device.rs` and the first Android
settings-action test overflowed a server worker stack. The retained core's mapped
GNU build ID matched the retained executable exactly. The runtime emitted an
explicit stack-overflow diagnostic; available memory and the core metadata did
not indicate an OOM kill. No unchanged failing test was retried.

The ADB action path reads five settings concurrently after each mutation. Its
common process runner still held two inline 16 KiB pipe buffers inside boxed
async work. Boxing the work did not prevent debug poll frames from reserving
space for those buffers and intermediate moves. The recently added stdin
coordinator introduced an additional nested read-join poll layer.

A compilation-only comparison used the complete process modules before stdin
coordination, with stdin coordination, and with bounded heap pipe buffers. The
layout probe never polled futures or launched a process. It ran only to print
sizes inside a private container; assembly was inspected without executing the
poll functions.

| Selected debug poll frame | Before stdin coordination | Inline buffers with stdin | Heap buffers with stdin |
| --- | ---: | ---: | ---: |
| `run_inner` | 36,808 | 37,368 | 4,648 |
| Outer work | 66,792 | 67,464 | 2,024 |
| Additional read-join work | No corresponding layer | 66,568 | 1,128 |
| Sum of these selected reservations | 103,600 | 171,400 | 7,800 |
| Public `run` future layout | 984 | 1,072 | 1,072 |

These are debug code-generation observations, not a complete reconstructed crash
stack. The core's original faulting call chain remained unresolved with the
container's library paths unavailable to the host debugger. The additional poll
layer and the heap-buffer comparison support stack pressure as the mechanism.

The production change replaces each fixed-size inline buffer with a zeroed
bounded vector of the same 16 KiB length. Output caps, read chunk size, stdin,
timeouts, cancellation, process-group cleanup and concurrency remain unchanged.
The vector initialization allocates directly on the heap instead of first
constructing a large stack array.

The existing Android action test now executes its unchanged typed reader/action
assertions in a fresh Linux child with pthread caching disabled at exec. Its
real in-process HTTP server measures both actual Tokio workers as 524,288 bytes.
The fake ADB commands exercise appearance, text size, networking and animation
mutations followed by all five actual settings readers. An independent process
regression also checks null stdin, both output-cap policies, stderr preservation,
timeout and cancellation. Fixtures stay private, retain their directories for
container teardown, and never run a real Android SDK or host ADB.
