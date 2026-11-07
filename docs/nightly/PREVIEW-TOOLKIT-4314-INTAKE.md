# Native preview toolkit intake

Source: nightly4314, `83a3674ffd84ffb33de768210c806cb1ac53fd80`. This document is an implementation proposal, not an enabled capability or a runtime proof. The isolated branch starts at9ca6a01 and preserves the separately proven keyboard implementation. No existing public toolkit metadata is expanded by this intake.

## Native surface

| Native tool | Dispatch and indispensable behavior | Foundation boundary |
| --- | --- | --- |
| preview_status | Thread-local tab/profile listing, controller/generation, loading, dialogs, chooser, downloads and viewport | Existing Rust authority must remain authoritative; normalize to actual native schema |
| preview_open | Captured preview capability, actual thread tab selection/reuse, profile and optional environment target navigation | Existing owned Pool and broker, not arbitrary worker-created tabs |
| preview_dialog | Owned pending dialog response | Existing dialog admission plus actual Playwright/CDP state |
| preview_navigate | Target resolution, readiness, navigation errors/download semantics | Source-backed orchestration around owned page |
| preview_resize | Viewport mode/settings/result and deadline | Actual native codec and viewport integration |
| preview_set_appearance | Color scheme state/emulation | Explicit per-tab operation; no attach-time global override |
| preview_snapshot | Visible text, accessibility tree, refs, console/network/action timeline, masked screenshot | Execute whole native ServerBrowserPage snapshot; innerText/CSS fallback is insufficient |
| preview_act | Separate strict action decode for click/type/hover/select/drag/upload/press/scroll | Execute actual Playwright locator methods; chooser integration remains an explicit gap |
| preview_evaluate | CDP promise/value flags, termination deadline, exception mapping and64KB value limit | Whole native evaluate function, owned page CDPSession only |
| preview_wait_for | All requested locator/text/URL conditions and native timeout/poll behavior | Whole native waitFor function |
| preview_recording_start | Owned scratch encoder page, idle first frame, screencast, pointer rendering, frame accounting | Page encoder script alone is insufficient |
| preview_recording_stop | Drain/encode, bounded artifact, attachment transfer/claim and thread ownership | Source-backed recording orchestration and owned artifact publication required |

The toolkit's flat preview_act schema is not the action validator: handlers decode again through PreviewAutomationActInputs[action]. Metadata descriptions, annotations, input/success/failure codecs and handler results must come from the actual native definitions. Snapshot strips presentation-only includeImage/save before broker dispatch, and image/file handling occurs in its specialized toolkit layer. Evaluate wraps result under value; ordinary object operations preserve optional website toolIcon. The current legacy preview_click/type/press aliases do not substitute for this catalog.

## Proposed runtime and packaging

Use an optional private Node worker with the exact ServerBrowserPage.ts and its SessionControl.ts error dependency, native contracts and Effect4.1.2; pin playwright-core1.60.0 to the native pnpm integrity. Preserve T3 MIT copyright/license, Playwright Apache2 LICENSE/NOTICE/third-party notices and Effect license. Verify the complete installed file inventory, not only the keyboard reference's selected bundle. Normal tool acquisition/build is separate from private runtime tests; runtime is offline and uses no ambient HOME, auth, token or profile. Node version/type-stripping and all dependencies must be sealed and tested before any capability is enabled.

Do not import ServerBrowser.ts/ServerBrowserContexts.ts wholesale: they include profile-tree cleanup and broader process/profile ownership. Reuse whole safe page operations, and intake each required orchestration method separately with explicit ownership adapters. No private Playwright patch or private API transport shortcut is proposed.

Rust retains captured MCP preview capability, policy/scope checks, thread/tab targeting, existing SessionControl receipts/generation, activity and lifetime ownership. Worker requests carry only admitted operations for a concrete grant; the worker is not a second authorization authority. A crashed worker makes the capability unavailable and cancels its in-flight operations without replaying mutations.

## Tab-scoped CDP relay

Local Pool reuses one Chromium process by profile across thread/tab keys. A raw browser pipe or endpoint therefore exposes other threads and is prohibited. A scoped relay must enumerate/attach only the granted target and its explicitly proven descendants, rewrite/filter browser-level target events, and reject unrelated target/session/context IDs at the transport boundary. Browser.close, arbitrary Target.createTarget, browser-context creation/disposal and arbitrary download-path changes are not model-callable. Scratch recording targets require a separate owner-issued grant and cleanup receipt.

The existing desktop Channel endpoint supplies a private single-client nonce-bearing loopback connection and lifetime cancellation; inspect its page boundary rather than assuming a private URL alone supplies target isolation. The local bridge must use the existing owned pipe/multiplexer, not discover another debugging endpoint. The endpoint URL must never enter logs/public state. Stop the worker and fence sessions before releasing its grant; old grants cannot close or mutate replacements.

Playwright public connectOverCDP supports noDefaults:true, which preserves existing download/focus/media defaults. Without it, context initialization calls Browser.setDownloadBehavior. Do not silently acknowledge that global mutation: use the supported option and preserve existing Rust-owned allowAndName download handling.

## Transitive filesystem boundary requiring review

Even connectOverCDP(noDefaults:true) always creates a private artifacts directory and calls removeFolders recursively on disconnect or failed connection. This is present in the unchanged official coreBundle.js; attach is not filesystem-free. No runtime test may invoke that recursive filesystem operation. A guard must be installed before importing Playwright.

Proposed explicit retaining adapter: register only Playwright-created artifact directories under a dedicated0700 private temp root, preserve their inode identity, and route their exact cleanup request to an identity-checked atomic rename into retained private quarantine. Verify identity after rename, perform no traversal or deletion, and fail for an unknown/nonowned path. All other recursive-removal requests remain fail-closed. This is a visible ownership adapter with path-removal/retention semantics, not a no-op or package patch. Tests must qualify quarantine namespace removal separately from physical deletion. Production retention/quota behavior requires review before activation. Killing a worker before ordinary disconnect is insufficient because connection failures also trigger cleanup.

## Source oracle and physical proof sequence

1. Extract metadata/codecs from the actual native toolkit and contracts with canonical Effect identity; preserve annotations and strict per-action decode. Exercise boundaries without executing native tests that clean up directories.
2. Stage exact whole Page/SessionControl source and all pinned dependency bytes/licenses. Audit imports and transitive attach/teardown; validate the retaining adapter and fail-closed guard first.
3. Launch only private real Chromium in rootless Podman with read-only source, dedicated HOME/profile/artifact directories, no host display/profile/DBus, offline loopback fixtures and retained outputs. The harness owns Chromium; worker receives only its tab-scoped grant.
4. Prove real public Playwright attach, accessibility refs and strict locators, iframe/shadow behavior, masked password snapshots, navigation/timeout/stale/wrong-tab refs, actual click/type/hover/select/drag/press/scroll/wait/evaluate semantics. Compare whole native function outcomes, not a copied CSS reducer.
5. Attack the transport with another thread's target/session, guessed target IDs, context/browser mutations and target events. Prove refusal before upstream dispatch, including after replacement/revocation and during cancellation. Test owned child-frame admission independently.
6. Only then wire Rust production adapters and prove actual provider MCP dispatch. Upload chooser/recording/attachment paths need their own real private acceptance before claiming the full12-tool surface. No public catalog expansion or full readiness claim is justified by this foundation.

## Foundation implemented for review

The new runtime modules remain unregistered: they are not imported by Rust startup and do not change the public legacy tool catalog. `page-worker.mjs` is an in-process worker core, not yet a supervised Rust-launched subprocess. The owner supplies its admitted callback, exact pinned modules and explicit artifact boundary. Native action/evaluate/wait schemas are decoded through actual Effect before dispatch. A future production launcher must seal dependencies, supervise its process/IPC, and bind that callback to actual Rust authority receipts; tests use a private owner callback and do not prove those absent integrations.

The minimal staged reference has84 byte-exact files, including the entire native page module/error dependency and runtime contracts, excluding all native tests and profile-management modules. The complete Playwright package has106 manifest-verified files; its bundled ws/wsServer utility is used for the foundation relay, while browser attachment uses only public connectOverCDP. Packaging must retain all package licenses/notices and pin this utility dependency explicitly. Additional T3/Effect notices accompany this source intake.

The relay supports a virtual browser-session namespace for public newCDPSession without granting a raw shared-profile browser session. Root target discovery is filtered, page sessions are tracked, and privileged browser/context/download/file-upload commands are refused. Child iframe/worker events are admitted only from an already owned parent session; real out-of-process child-frame and popup/scratch grants still need dedicated proof and are not certified by same-process iframe tests. Unknown root/foreign events remain filtered. Per-slot artifact cleanup is serialized because native disconnect and close can request the same cleanup concurrently.

Real private Chromium tests exercise whole native click/type/hover/select/drag/press/scroll/wait/evaluate/snapshot, same-process iframe locators, masked password/a11y/PNG output, stale/wrong-tab refs, strict-action codec rejection and transport refusal/revocation. These selected actions are a foundation; status/open/resize/appearance/dialog orchestration, chooser upload, recordings and real Rust provider-MCP dispatch remain open. Snapshot diagnostics arrays are explicit empty fixture inputs here; production diagnostic/timeline collection is not implemented by this core.

All future aliases, especially preview_press, must share the same native Page keyboard state with preview_act or be explicitly retired. The existing Rust keyboard remains the currently active legacy implementation; running both independent keyboard states against one activated page would be incorrect.
