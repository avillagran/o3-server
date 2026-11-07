# Frontend-owned desktop page adoption

`Browsers::open_desktop` connects only to an endpoint created by the owned desktop
channel. It never accepts an RPC-supplied debugging URL. The relay uses an actual
secret loopback WebSocket; endpoint URLs and CDP payloads are not logged.

Nightly 4288 `ServerBrowserContexts.ts:235` connects over CDP and selects the first
existing page, failing with `The desktop tab has no page.` when none exists. The
O3 actor expresses this through `Target.getTargets`, first `type: page`, flattened
`Target.attachToTarget` and `Page.enable`. It neither creates a target nor performs
an initial navigation. Page commands retain the opaque flattened session ID;
browser and target commands remain browser-level. Response IDs are correlated,
messages are bounded to 4 MiB and commands retain the existing five-second actor
and six-second caller deadlines.

Desktop origin is explicit. Local process reuse excludes desktop tabs. Local
profile clearing does not stop a desktop actor. Explicit desktop close/shutdown
closes the connection and releases the channel endpoint, without `Browser.close`,
`Target.closeTarget`, process signals or profile removal. A matching channel detach,
CDP detach or socket disconnect ends that connection. The tab's desktop origin and
completion state remain inspectable so the coordinator can retain its preview
session and reconnect with a fresh endpoint. This does not reconnect automatically.

Focused tests drive the real channel, loopback relay and WebSocket actor against a
synthetic CDP peer. They verify first-page selection, opaque session routing,
non-destructive close, missing-page failure and detach/reconnect. They launch no
browser and touch no browser profile. Actual native relay reference acceptance is
tracked separately; these tests do not claim Playwright or real frontend runtime
acceptance.

The preview coordinator owns attachment waiting, publication order, pending-open
cancellation, frontend control and pointer subscriptions. This actor checkpoint
changes no startup capability or desktop bootstrap admission.
