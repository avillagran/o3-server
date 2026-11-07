# Planned owned Page CDP lane

This is an implementation boundary plan, not an enabled transport or parity claim. The persistent worker/control adapter is separately tested. No public toolkit or alias selects it yet.

## Captured authority

A binding must be retained by the actual `server_browser::Tab`, with its immutable thread/tab transport identity, owned target, current transport generation and existing Control. A new target gets a fresh binding. Ordinary controller transitions keep the binding, Page and Keyboard and invalidate refs through the private owner command. Once a tab selects the native driver, all its published legacy aliases must use that driver; a worker failure cannot fall back to the independent Rust keyboard or CSS helpers.

Local Chromium is shared by profile and uses the existing inherited pipe. Desktop adoption uses the channel-owned transport. Neither grants a model access to a raw browser endpoint. The owner bridge must retain the existing bounded actor/writer and introduce an explicit owned raw-request route, rather than globally changing the handling of Target commands or their deadlines. Child-session Target.setAutoAttach requires its actual session; the legacy route currently strips Target/Browser session IDs, so reusing it unchanged is incorrect.

## Exact JSON routing

A raw envelope parses method, request ID and session identity independently of RawValue-backed params/result/error. The actual process-wide correlation owner assigns IDs; worker and legacy requests cannot collide. An owned raw response is dispatched by its recorded request ID before the legacy scalar decoder. Owned worker-session events are dispatched by the captured session registry. Existing unrelated scalar requests/events retain their existing decoder and policy; no global parser relaxation or lossy replacement is proposed.

The Rust authority registry must admit only the captured target, its actual attached sessions and child iframe/worker targets reported through an already owned parent session. Root target/browser discovery, context creation/closure, browser closure, arbitrary download paths and unowned sessions remain unavailable. Target lifecycle authority is parsed from exact scalar identity fields separately from application titles, URLs and other raw fields. Global events cannot enumerate other targets. A worker-provided target, session or deadline is never itself an ownership receipt.

The Node relay continues to enforce its independent scope and late-attachment cleanup. Its upstream is the Rust-owned lane, not a raw shared-profile endpoint. The raw lane must preserve actual CDP errors and exact application JSON rather than fabricate successful unsupported initialization commands.

## Deadlines and cleanup

The existing legacy command deadline remains unchanged. The new raw route needs explicit bounded startup/housekeeping permits and admitted per-operation permits; it cannot silently inherit the legacy five-second command clamp for a native evaluation whose admitted timeout is longer. The Rust broker owns each permit and its deadline. Concurrent observer calls cannot extend an unrelated mutation's permit. Native operation time and the original1000ms host-response grace remain distinct, including time spent queued for control.

Close first fences new admission, drains owned in-flight attachments, and detaches every confirmed late grant. Expired attachment/detachment or browser-execution outcomes remain unknown; a reaped Node process alone does not prove browser-side effects stopped. Only the captured grant/worker may be canceled, never another target or the shared Chromium process. Cleanup receipts remain retained with the tab until actual transport/operation ownership is resolved.

## Admission tests before activation

The next physical acceptance must traverse a synthetic provider's real MCP HTTP connection, actual authority and broker, actual tab owner/control, distinct native worker and owned CDP lane into private Chromium. It must prove cross-thread target/session refusal before transport access, human/foreign-agent rejection, one keyboard across aliases/handoffs, stale ref rejection, queued deadline behavior, late response fencing and ambiguous attachment cleanup.

Escaped isolated UTF-16 units need both directions: raw arguments into the native codec and exact results through the actual native success codec and the outer MCP JSON response. The complete Effect MCP registration oracle already proves that native output preserves both structuredContent and text. Omitting or replacing either with null is not an equivalent consumer. Recording/scratch ownership, chooser/uploads, diagnostics, status/open/dialog/resize/appearance and the full twelve-tool dispatch remain separate required consumers; a transport foundation does not close them.

## Exact-envelope prerequisite now implemented

`native_page_worker::cdp::Packet` is a separate, inactive raw-envelope type. It retains validated RawValue-backed object params/result/error, parses scalar control fields independently, rejects duplicate or ambiguous control fields and unsafe IDs, and serializes directly without an intermediate Value. Its read-only fields do not themselves grant target/session authority. A caller-supplied byte cap is checked before parsing; the eventual transport must enforce its admitted cap before receiving/allocating the body. No existing pipe parser or actor has been changed by this prerequisite.

Private tests cover command, event, result and error payloads with isolated UTF-16 units, plus malformed/duplicate/spoofed envelope fields. A real private Chromium probe evaluates an isolated unit and an object with isolated key/value units through the actual native Page and existing Control adapter. The fixture retains only those two explicitly synthetic Runtime.evaluate response packets, returns them over the owner-authenticated private test controller, and requires Rust raw-envelope decode/serialization/decode to preserve both. This proves the exact-envelope representation against actual browser output; it does not prove the planned raw lane is connected to the production pipe or authenticated MCP dispatch. The legacy scalar decoder remains unchanged.
