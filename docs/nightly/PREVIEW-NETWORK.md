# Browser network diagnostics and navigation failures

The server maps owned page-session CDP notifications into the network diagnostics and navigation failure state from nightly 4288 (`32a0531748c44b25072d7495ce0b9bd7e9450a87`). Both inherited-pipe local pages and adopted desktop pages use the same identity-fenced observer. Local listeners are installed before the initial navigation.

`tools/generate-preview-network-fixtures.mjs` executes the native request, response, request-failed, load, frame-navigation and aborted-navigation callbacks with inert page, clock and timer services. Its 263 traces cover the 200-entry diagnostic bound, network error mapping, navigation generations and the five-second aborted-navigation settlement. The download-offer cases exercise settlement only; they do not save files.

The raw event adapter uses the pinned Playwright 1.60.0 navigation predicate and fragment stripping. Another 192 cases execute those exact upstream source fragments. Requests for `data:` URLs are ignored, responses retain their original request URL and method, and unknown request IDs do not create diagnostics. Source hashes and exact upstream URLs are recorded in the fixture. The source boundaries are [Chromium network management](https://github.com/microsoft/playwright/blob/v1.60.0/packages/playwright-core/src/server/chromium/crNetworkManager.ts) and [request URL handling](https://github.com/microsoft/playwright/blob/v1.60.0/packages/playwright-core/src/server/network.ts).

The existing `preview_snapshot` value includes native-shaped `networkEntries`. Its existing output-size bound and legacy DOM result remain in place. This does not implement the native full DOM reference snapshot.

A failure for an older main request remains in diagnostics but cannot replace a newer navigation status. Subresource requests do not alter main navigation state. Aborted navigation waits for the native five-second settlement window; owned completion monitors cancel with page closure and cannot revive a closed session.

Child-frame, worker and out-of-process-frame session migration are not implemented. Dialog, popup and saved-download consumers remain separate work. Generic CDP transport and diagnostic visibility do not establish those capabilities. Desktop download placement also requires the client to prove actual file creation before any saved-file offer can be published.

The pipe acceptance peer emits fragmented real protocol notifications, including a stale main-request failure, a newer DNS failure, 205 completed subresources and an aborted main request. It verifies the final 200 records, original request URLs without fragments, subresource/status separation and the measured five-second abort timer. Actual MCP agent peers verify that snapshot output carries `networkEntries` while retaining the page value and admission rules.

Observer removal and transport completion remain distinct. An explicit last-page close cannot be mistaken for an unsolicited detach by an unrelated in-flight response. Reattaching a desktop page joins the previous endpoint owner before creating its replacement, so the old release cannot invalidate the new endpoint.
