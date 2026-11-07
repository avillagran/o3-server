# Local browser notification ownership

A dedicated reader continuously consumes the inherited Chromium CDP pipe. It
retains NUL framing across fragmented reads, validates each bounded JSON packet,
and delivers at most eight pending packets to the actor. It remains active while
no command is waiting. EOF, truncation, invalid JSON and messages reaching the
existing 4 MiB limit end the owned connection. Actor finalization aborts and joins
the reader before process/profile cleanup.

Responses retain command-ID correlation and five-second active/six-second caller
deadlines. The [transport foundation](BROWSER-TRANSPORT-LIMITS.md) adds bounded
concurrent command correlation and owned writes so dialog answers can unblock
held evaluations. Unsolicited events use the same source-proven
navigation projection and strict session, target and main-frame routing as the
desktop transport. Each local page has its own observer and completion watch;
a shared profile does not share page identity. A separate process-completion watch
acknowledges actual process reaping and existing identity-checked cleanup.

A detached/destroyed page stops only its observer. A live sibling keeps its shared
process and remains controllable. When the final live page closes, stale detached
tab metadata cannot defer process reaping. Closing an already-detached page sends
no stale target-close command; closing a live owned page still sends its command.
Thread cleanup also removes stale actor entries without stopping another thread's
live page. Existing production profile-removal behavior and explicit fixture
removal injection remain unchanged.

The physical test uses an actual inherited pipe and owned Python peer. Three
independent pages share one process. Delayed fragmented notifications arrive
while the actor is idle, update only the matching page, and ignore unrelated
sessions. External page closure preserves a live sibling, thread cleanup remains
isolated, and the final close verifies the real child PID was reaped. All fixture
trees remain present. Pure reader tests cover framing, truncated input and the
message bound; observer tests cover per-page closure. Existing preview, desktop,
agent and deletion tests provide adjacent acceptance.

The subsequent [network increment](PREVIEW-NETWORK.md) adds bounded diagnostics
and request-failure projection. Console, dialog, popup and download consumers
remain separate work. No capability is inferred from generic CDP command forwarding.
