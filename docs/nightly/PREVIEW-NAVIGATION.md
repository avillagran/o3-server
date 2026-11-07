# Desktop page notification projection

Source pin: nightly 4288, `32a0531748c44b25072d7495ce0b9bd7e9450a87`.
The reference generator executes the exact `report` and `reportLoaded` bodies from
`ServerBrowser.ts:581–619`, the complete `withoutDataPayload` helper, and the complete
native desktop `CdpRelay` with inert page/debugger services. The retained corpus
contains 409 report observations, 11 redaction cases and two relay event-gating
traces. Source hashes and execution scope are embedded in the fixture.

The actor enables discovery and automatic attachment after selecting its existing
page. The native relay discards page events until `Target.setAutoAttach`; a manual
attachment alone does not open that event gate. Automatic attachment broadcasts
main-page events to its opaque page sessions. O3 observes only its own flattened
session, the selected target, and the main frame established by `Page.getFrameTree`.
Child-frame and unrelated-session events cannot update that preview.

Matching load, same-document, main-frame and target-title notifications advance a
bounded watch state. Navigation starts advance its generation and keep the loading
fence set until the page load event. The owned completion monitor synchronizes
initial metadata and subsequent notifications without a listing/viewer request.
It reads actual history and document metadata, checks the watch generation again,
and verifies that the same live actor still owns the watch before committing.
An old actor therefore cannot publish into its replacement after reattachment.
Explicit close still removes the kernel session before runtime cleanup.

Loaded reports preserve native guards: blank/error pages do not clear an existing
failure; changed navigation, loading and changed URLs suppress stale success;
URLs and titles are bounded; data URLs expose only their media type. The kernel
receives trusted controlled reports, and its finalized events wake subscribers.
Identical reconciled metadata is coalesced as in the existing coordinator.

Actual loopback tests drive initial metadata, title and same-document changes,
unrelated session/target/child-frame suppression, error-page failure retention,
data redaction and truncation, navigation beginning during a title read, and close
without resurrection. Separate Rust checks consume the actual native relay frames
and verify its opaque session routing. The relay oracle uses the complete native
module; the socket fixture uses a synthetic protocol peer. No real browser, SDK,
profile or desktop service is exercised.

This checkpoint implements desktop loaded-metadata notification projection. Local
inherited-pipe event consumption still uses the existing listing/viewer sync path.
Native request-failure/download/network/console/dialog/popup projections are
separate consumers; this proof does not claim them or enable additional capabilities.
