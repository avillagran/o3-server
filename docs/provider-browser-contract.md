# Provider updates and server browser contract (2026-11-02)

Reader inventory (local source; not pinned binary evidence):

- Desktop `src/provider_updates.rs::advisory` reads enabled, driver,
  compatibilityAdvisory.latestVersionStatus, versionAdvisory.status/currentVersion/
  latestVersion/canUpdate/updateCommand and updateState.status/message. Only
  behind_latest, enabled, compatible providers enter its panel. Queued/running
  disable one-click. `run_update` uses lib `api::provider::update`, with the long
  RPC deadline, then config recovery on uncertain outcome.
- Lib `provider_instances::projection` reads setup.canInstall/canAuthenticate,
  auth.canLogout and version advisories. Desktop provider setup uses install
  subscribe/start/cancel/remove; external updates now publish an operation ID to
  this existing subscription, while config publishes updateState.
- Desktop `browser_profiles.rs::inspect/apply_with_at` reads
  environment.capabilities.serverBrowser and authenticated permissions. It calls
  lib `api::preview::clear_profile` only for capable environments, and requires
  successful acknowledgement before deleting local metadata.
- Lib `api::preview` validates open/list/navigate/resize/adjust/refresh/close and
  clearProfile. Preview snapshots contain threadId, tabId, runtime, profileId,
  navStatus, viewport, timestamps and history flags. The authenticated media
  endpoint is `/api/preview-stream/ws`, using media tickets or bearer sessions;
  frames are binary JPEG, paced by JSON ack. Basic control/input is implemented.
- Android `ProviderAdministration.refresh` keeps instanceId/displayName/driver/
  enabled/installed/version; setup.canInstall/canAuthenticate, auth.status/
  canLogout, versionAdvisory.status/currentVersion/latestVersion/canUpdate.
  `act(Update)` explicitly sends provider=driver and instanceId. Setup refresh uses
  provider.installState/authState; cancellation uses the returned operationId.
  No Android browser-stream/profile UI call site was found. This is source
  inventory, not a running UI assertion.
- Desktop `preview_transport::snapshot/list/event` additionally validates optional
  colorScheme, zoomFactor, automationOwner, reveal/revealRequest and download
  fields. `preview_actions::execute` rechecks origin/epoch, preview:operate and
  serverBrowser before opening runtime=server. These controls already existed;
  their complete browser effects are not certified by the new fixture.

Implementation limits:

External automatic updates recognize only the standard, user-owned npm global
layout for the two native drivers, with matching package.json identity, original
prefix and its npm executable. Unknown, system-owned and Homebrew layouts are
manual advice, never self-update guesses. Other drivers remain manual. External
version pinning is rejected. Updates serialize by refusal of a simultaneous job,
not a durable queue. A version response proves the executable still runs, not
that registry latest was installed. No real package manager update was run.

The browser is a first implementation, not completed agent-browser compatibility.
System executable discovery is lazy; absence gives an installation message. CDP
uses an inherited Unix pipe, exposing no debugging TCP port. Each active profile
has a dedicated browser process and attached page; eight processes maximum,
30-minute lifetime, five-second command deadline, bounded CDP messages. Profiles
are hash-named inside the o3 profile directory; incognito gets temporary storage.
Close, clear and shutdown signal the actor, kill/reap its child and process group.
Clear waits for process exit, rejects profile-root symlinks and closes snapshots.

Still required: agent automation broker/tool registration (agentBrowserAccess
remains false), multiple tabs per persistent context, controller arbitration,
complete navigation/load/history event synchronization, downloads/uploads,
dialogs, clipboard/probe and recording, browser crash recovery, and wider
security/lifetime race coverage. Stream screenshots use captureScreenshot rather
than Chromium screencast events. System Chromium sandbox/dependency startup and
actual page rendering were not tested. The fixture child is Python, whose RSS
must never be represented as Chromium memory consumption.

Newer upstream source uses Playwright contexts and a maintenance resolver; these
are source-only observations. Existing pinned differentials do not establish
external updater or browser engine equivalence. Statuses and priorities stay
unchanged pending independent audit.
