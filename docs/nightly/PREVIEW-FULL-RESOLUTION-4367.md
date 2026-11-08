# Native full-resolution Page capture

The Page source manifest now selects Nightly 4367,
`6417151c093e54dbba0e6c824c69d03983793b9a`. All 84 selected original files
are byte identical to 4353, which added optional full-resolution capture.
Nine of those files differ from the preceding 4314 runtime manifest. The
separate toolkit manifest retains its earlier scope and revision.

The Page worker now decodes the actual `PreviewAutomationSnapshotInput`
before dispatch and forwards its optional `fullScreenshot` field. The complete
original Page function returns the capped agent PNG and, only when requested
and larger, its original full PNG. Smaller captures avoid a duplicate image.
`tools/stage-native-preview-page` stages the exact manifest's original Git
blobs into a new source directory without switching the original checkout or
executing package scripts. The original MIT license is preserved.

The private physical probe passes six cases against real scoped Chromium:

- Invalid input is rejected before any CDP call.
- A 2048x1024 viewport returns both its full PNG and a 1280x640 agent PNG,
  with actual decoded color pixels preserved.
- Omitted and explicitly false options return only the capped image.
- A 640x480 viewport avoids an unnecessary full-image duplicate.
- The complete native function at an actual 2x device scale retains
  2560x1600 pixels while its agent image remains 1280x800.

The worker candidate tested by that probe is byte identical to the integrated
worker, SHA256 `4f2f10c073f72a333bcd7b6cc21b6196470409ba7ef7d6789c76ed47d7bc8694`.
The high-DPI case calls the complete original function with its actual 2x
scale; it does not claim that the current 1x worker infers a different scale.

Evidence is retained at
`/var/tmp/o3-server/preview-full-capture-4367-3-0qwae7_h/run1/result.json`,
SHA256 `46c5399c3f05e5cf78cd257eb59724b5b2413843e5d889e372438ce03452d539`.
The same client and retained container ID reached terminal state, Chromium
exited normally, all 2551 inputs stayed unchanged, and the recursive-removal
guard recorded zero refusals. Playwright artifact requests were retained by
the existing boundary; no recursive removal, Cargo, new target or runtime copy
was executed. Earlier attempt 1 failed while mounting a missing empty module
directory; attempt 2 rejected an unsupported Node option before the probe ran.
Both failure receipts are preserved separately.

This is Page capture and typed worker forwarding acceptance. The fixture's
diagnostic arrays are explicit empty inputs. Public snapshot routing remains
closed until real diagnostics/status are complete. MCP screenshot-file saving,
stripping the full image from structured output, and retaining the capped
inline image still require their actual caller-to-file proof. No public
selector, installed application or complete Nightly parity is accepted here.
