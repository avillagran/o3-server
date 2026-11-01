# P0/P1 field consumer audit

Source: retained nightly 3991 shape report; not a new differential run. Grep scans all text file types, including hidden/ignored source files and bindings/data, using camelCase, PascalCase, snake_case and kebab-case spellings. Hits are candidates, not proof of a read; accepted exceptions are reviewed in CONFORMANCE.md.

| Field | lib-t3-rust | desktop | Android |
|---|---|---|---|
| `_tag` | `src/api/assets.rs`, `src/api/diagnostics.rs`, `src/api/preview.rs` … | `src/browser_guest.rs`, `src/browser_preferences.rs`, `src/browser_runtime.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeT3Transport.kt` |
| `account` | `src/api/device.rs`, `src/api/pull_requests.rs`, `src/connection_catalog.rs` … | `src/connection_github.rs`, `src/desktop_cloud.rs`, `src/host_sync.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/notifications/NOTICE.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/connections/P13-SDK-GAPS.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/composer/INTEGRATION.md` … |
| `activeAttemptId` | `src/queue.rs` | no match | no match |
| `activeProviderThreadId` | `src/checkpoint_revert.rs`, `src/compaction.rs`, `src/queue.rs` … | `src/lib.rs`, `src/session_memory.rs` | no match |
| `activityRunStartedAt` | `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `activityRunStatus` | `src/awareness.rs`, `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | no match |
| `addProjectBaseDirectory` | no match | `src/write.rs` | no match |
| `agentActivityPublishing` | no match | no match | no match |
| `aliases` | no match | `src/build_cleanup.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/js/QtKeybindings.js` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `altKey` | no match | `src/keybinding_settings.rs`, `ui/js/NativeKeybindings.js`, `ui/js/QtKeybindings.js` … | no match |
| `appThreadId` | `src/compaction.rs`, `src/queue.rs` | `src/session_memory.rs` | no match |
| `archivedAt` | `src/compaction.rs`, `src/sidebar/view.rs` | `src/archive_management.rs`, `src/lib.rs`, `src/session_memory.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `archivedThreads` | no match | `src/archive_management.rs`, `src/shell_state.rs`, `ui/qml/T3Archive.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `attachmentUploads` | no match | no match | no match |
| `attemptOrdinal` | no match | no match | no match |
| `attempts` | `src/projection.rs`, `data/i18n.json` | `src/remote-bootstrap.sh`, `src/remote-restart.sh`, `src/remote-update.sh` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/reader/P08_VERIFICATION.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/ProviderRefreshGate.kt`, `app/src/main/res/values/strings_shared_drafts.xml` |
| `auth` | `src/api/assets.rs`, `src/api/provider.rs`, `src/api/pull_requests.rs` … | `src/archive_management.rs`, `src/browser_address.rs`, `src/browser_cookies.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `autoPull` | `src/api/projects.rs`, `src/native_settings.rs`, `src/projects.rs` … | `src/lib.rs`, `ui/qml/T3Preferences.qml`, `ui/qml/T3ProjectMenu.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectConfiguration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectSettingsScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/README.md` … |
| `autoResumeLimitedThreads` | `src/native_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `autoSettleDisabledAt` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3ThreadRow.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/SidebarView.kt` |
| `automaticGitFetchInterval` | no match | no match | no match |
| `backgroundActivity` | `src/native_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `backgroundActivityProfile` | `src/native_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `blockedOnThreadWait` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | no match |
| `branchNameInstructions` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `branchNamePrefix` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `branchNamingMode` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `browserArtifactsAfterDays` | `src/storage_settings.rs` | `ui/qml/T3StorageSettings.qml` | no match |
| `canAuthenticate` | `src/provider_instances.rs` | `src/provider_login.rs`, `src/write.rs`, `ui/qml/T3ProviderInstances.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `canInstall` | `src/provider_instances.rs` | `ui/qml/T3ProviderInstances.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `canInstallVersion` | no match | no match | no match |
| `canList` | no match | no match | no match |
| `canLoad` | no match | `src/review_document.rs`, `ui/document/diff-renderer/main.js` | no match |
| `canLogout` | `src/provider_instances.rs` | `ui/qml/T3ProviderInstances.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `canResume` | no match | no match | no match |
| `canUpdate` | `src/provider_instances.rs`, `src/server_updates.rs` | `src/provider_updates.rs`, `src/server_update.rs`, `ui/js/ProviderUpdates.js` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `capabilities` | `src/domain/mod.rs`, `src/domain/settings.rs`, `src/ffi.rs` … | `src/browser_profiles.rs`, `src/connection_access.rs`, `src/lib.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/environments/BridgeAdministration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/DesktopSurfacesPanel.kt` … |
| `checkedAt` | `src/domain/usage.rs`, `src/quota_present.rs`, `src/quota_sources.rs` … | `src/local_quotas.rs`, `src/quotas.rs`, `src/server_update.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageLimits.kt` |
| `checkpointScopeId` | no match | no match | no match |
| `codex` | `src/api/ids.rs`, `src/compaction.rs`, `src/provider_instances.rs` … | `src/agent_sessions.rs`, `src/host_sync.rs`, `src/journal_vault.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ComposerModels.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/QuotaStrip.kt` |
| `command` | `src/activity.rs`, `src/api/ids.rs`, `src/api/mod.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/ThreadFollowService.kt` … |
| `commandOutputStreaming` | no match | no match | no match |
| `compactIdleThreads` | `src/domain/settings.rs`, `src/native_settings.rs`, `bindings/android/describe.json` … | no match | no match |
| `completedAt` | `src/awareness.rs`, `src/compaction.rs`, `src/plan_workflow.rs` … | `src/build_cleanup.rs`, `src/lib.rs`, `src/session_memory.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `config` | `src/api/device.rs`, `src/api/projects.rs`, `src/api/provider.rs` … | `src/auto_compact.rs`, `src/backend_service.rs`, `src/browser_guest.rs` … | `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt`, `app/src/main/assets/licenses/androidx.activity.activity-compose-1.12.4.txt` … |
| `connectionProbe` | no match | no match | no match |
| `contextUsage` | `src/timeline.rs` | `src/lib.rs` | no match |
| `continuation` | `src/api/orchestration.rs`, `src/native_settings.rs`, `data/i18n.json` | no match | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralTextResources.kt`, `app/src/main/res/values/strings_general_settings.xml` |
| `continueThreadsAfterServerUpdate` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `countsForRun` | no match | no match | no match |
| `createdAt` | `src/api/orchestration.rs`, `src/api/project_clone.rs`, `src/approvals.rs` … | `src/archive_management.rs`, `src/build_cleanup.rs`, `src/lib.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/pullrequests/PullRequestsScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/TimelineReader.kt` … |
| `ctrlKey` | no match | `src/keybinding_settings.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/js/NativeKeybindings.js` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `currentValue` | no match | `ui/qml/T3Composer.qml`, `ui/qml/T3ModelOptions.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralSettingsScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ModelPresentation.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md` |
| `currentVersion` | `src/provider_instances.rs` | `src/provider_updates.rs`, `ui/js/ProviderUpdates.js`, `ui/qml/T3ProviderInstances.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `cursorKeychainUsageEnabled` | no match | no match | no match |
| `customInstructions` | `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `databaseMigration` | no match | no match | no match |
| `databaseStorage` | no match | no match | no match |
| `day` | `src/api/diagnostics.rs`, `src/api/scheduled_tasks.rs`, `src/domain/settings.rs` … | `src/build_cleanup.rs`, `src/journal_vault.rs`, `src/keybinding_commands.json` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/Routes.kt` … |
| `defaultAutoPull` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `defaultModelSelection` | `src/api/projects.rs`, `src/domain/settings.rs`, `src/native_settings.rs` … | `src/agent_sessions.rs`, `src/lib.rs`, `src/write.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/newchat/NewChatDraftViewModel.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectConfiguration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectSettingsScreen.kt` … |
| `defaultProjectScripts` | `src/git_controls.rs`, `src/scoped_settings.rs` | no match | no match |
| `defaultRuntimeMode` | `src/domain/settings.rs`, `src/native_settings.rs`, `src/parity.rs` … | `ui/qml/T3Backend.qml`, `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `defaultTheme` | no match | `ui/assets/mermaid/mermaid.min.js` | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/theme/Theme.kt` |
| `defaultThemeSetAt` | no match | no match | no match |
| `defaultThreadEnvMode` | `src/api/projects.rs`, `src/native_settings.rs`, `src/parity.rs` … | `src/lib.rs`, `src/write.rs`, `ui/qml/T3Preferences.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectConfiguration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectSettingsScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `deletedAt` | `src/compaction.rs`, `src/git_controls.rs`, `src/sidebar/view.rs` | `src/archive_management.rs`, `src/branch_picker.rs`, `src/lib.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `description` | `src/api/preview.rs`, `src/composer.rs`, `src/domain/mod.rs` … | `src/acp_registry.rs`, `src/native_settings.rs`, `src/omabridge_share.rs` … | `app/src/main/assets/licenses/jakarta.inject.jakarta.inject-api-2.0.1.txt`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/GitSurfaces.kt` … |
| `detail` | `src/api/device.rs`, `src/api/pull_requests.rs`, `src/approvals.rs` … | `src/device_screen.rs`, `src/failure_context.rs`, `src/integration_devices.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/LiveUpdates.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/Ot3Notifications.kt` … |
| `deviceHosts` | no match | `src/integration_devices.rs`, `ui/qml/T3EasHosts.qml`, `ui/qml/T3Integrations.qml` | no match |
| `deviceOnboardingCompleted` | no match | `src/integration_devices.rs` | no match |
| `displayName` | `src/composer.rs`, `src/domain/usage.rs`, `src/models.rs` … | `src/provider_updates.rs`, `src/ui_state.rs`, `ui/browser/browser-host.cpp` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/AppearanceSettingsScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ComposerModels.kt` … |
| `driver` | `src/api/ids.rs`, `src/api/server.rs`, `src/api/vcs.rs` … | `src/local_quotas.rs`, `src/provider_login.rs`, `src/provider_setup.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProviderConfigurationForm.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ComposerModels.kt` … |
| `easDeviceHosts` | no match | `src/integration_devices.rs`, `ui/qml/T3EasHosts.qml` | no match |
| `enableAgentBrowserAccess` | `src/native_settings.rs`, `src/scoped_settings.rs` | `ui/qml/T3Integrations.qml` | no match |
| `enableAgentDeviceAccess` | `src/native_settings.rs`, `src/scoped_settings.rs` | `src/integration_devices.rs`, `src/native_settings.rs`, `src/scoped_settings.rs` … | no match |
| `enableDeviceSupport` | no match | `src/integration_devices.rs`, `ui/qml/T3DevicePanel.qml`, `ui/qml/T3Integrations.qml` | no match |
| `enableProviderUpdateChecks` | `src/native_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `enabled` | `src/api/device.rs`, `src/api/orchestration.rs`, `src/api/scheduled_tasks.rs` … | `src/auto_compact.rs`, `src/checkpoint_operations.rs`, `src/connection_network.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `environment` | `src/api/diagnostics.rs`, `src/api/orchestration.rs`, `src/api/pull_requests.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/fira-code-OFL.txt`, `app/src/main/assets/licenses/firacode-nerd-OFL.txt` … |
| `environmentIcon` | no match | `src/connection_access.rs`, `src/write.rs`, `ui/qml/T3ConnectionAccess.qml` … | no match |
| `environmentThemes` | `src/api/streams.rs`, `bindings/android/describe.json`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` | no match | no match |
| `errorCode` | `src/errors.rs`, `src/ffi_call.rs`, `src/ffi_stream.rs` … | `src/browser_profiles.rs`, `src/device_screen.rs`, `src/integration_devices.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HeaderStatus.kt` … |
| `executable` | `src/activity.rs`, `data/i18n.json`, `data/provider-drivers.json` | `src/backend_service.rs`, `src/browser_guest.rs`, `src/browser_runtime.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/DesktopSurfacesPanel.kt`, `app/src/main/res/values/strings_provider_fields.xml` |
| `executionRequested` | no match | no match | no match |
| `fallbackModelSelection` | no match | no match | no match |
| `faviconPath` | `src/api/projects.rs`, `src/assets.rs`, `src/sidebar/view.rs` | `src/lib.rs`, `src/notification_icon.rs` | no match |
| `favorites` | `src/domain/mod.rs`, `src/models.rs`, `bindings/android/describe.json` … | `src/write.rs`, `ui/qml/T3Backend.qml`, `ui/qml/T3ModelPicker.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/newchat/NewChatDraftScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ComposerModels.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` … |
| `fileAttachments` | no match | no match | no match |
| `firstRunOrdinal` | no match | no match | no match |
| `followChangeRequestTemplates` | `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `forkedFrom` | `src/timeline.rs` | `src/lib.rs` | no match |
| `goal` | `src/domain/mod.rs`, `src/sidebar/view.rs`, `src/sidebar.rs` … | `ui/assets/lucide-project-icons.json`, `ui/assets/mermaid/mermaid.min.js`, `ui/js/SidebarModel.js` … | `app/src/main/assets/licenses/fira-code-OFL.txt`, `app/src/main/assets/licenses/firacode-nerd-OFL.txt`, `app/src/main/assets/licenses/font-awesome.txt` … |
| `groupKey` | `src/sidebar/view.rs` | `ui/assets/mermaid/mermaid.min.js`, `ui/js/NativeKeybindings.js`, `ui/js/ProjectModel.js` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `guestSharingEnabled` | `src/native_settings.rs` | `src/write.rs`, `ui/qml/T3Preferences.qml`, `ui/qml/T3ThreadDetails.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `handoffIds` | no match | no match | no match |
| `host` | `src/api/agent_sessions.rs`, `src/api/device.rs`, `src/api/diagnostics.rs` … | `src/auto_compact.rs`, `src/browser_address.rs`, `src/browser_cookies.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `id` | `src/activity.rs`, `src/api/agent_sessions.rs`, `src/api/assets.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `idleCompaction` | `src/compaction.rs`, `src/domain/settings.rs`, `src/native_settings.rs` … | `src/auto_compact.rs`, `src/native_settings.rs`, `src/scoped_settings.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `ignoredFileListing` | no match | no match | no match |
| `inlineMessageContext` | no match | no match | no match |
| `input` | `src/activity.rs`, `src/api/assets.rs`, `src/api/device.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/assets/themes/omarchy-themes.json` … |
| `installHint` | no match | no match | no match |
| `installed` | `src/domain/usage.rs`, `src/models.rs`, `src/native_settings.rs` … | `src/backend_service.rs`, `src/browser_sources.rs`, `src/connection_network.rs` … | `app/src/main/assets/licenses/index.json`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralTextResources.kt` … |
| `instanceId` | `src/api/ids.rs`, `src/api/provider.rs`, `src/api/scheduled_tasks.rs` … | `src/auto_compact.rs`, `src/checkpoint_operations.rs`, `src/connection_route.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/newchat/NewChatDraftScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralSettingsScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectSettingsScreen.kt` … |
| `isCustom` | no match | `ui/assets/mermaid/mermaid.min.js` | `app/src/main/assets/mermaid/mermaid.min.js` |
| `isDefault` | `src/branch_picker.rs`, `src/git_controls.rs`, `src/provider_instances.rs` … | `src/remote_surfaces.rs`, `src/working_review.rs`, `ui/assets/mermaid/mermaid.min.js` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/GitSurfaces.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProviderConfigurationForm.kt` |
| `isLegacy` | `src/models.rs` | `ui/qml/T3ModelPicker.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ComposerModels.kt` |
| `item` | `src/activity.rs`, `src/api/assets.rs`, `src/api/ids.rs` … | `src/browser_cookies.rs`, `src/browser_sources.rs`, `src/command_output.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/connect/ConnectScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/environments/EnvironmentsScreen.kt` … |
| `key` | `src/api/assets.rs`, `src/api/mod.rs`, `src/api/orchestration.rs` … | `src/archive_management.rs`, `src/auto_compact.rs`, `src/browser_cleanup.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/LegacyNotificationExpiry.kt` … |
| `keybindings` | `src/api/server.rs`, `src/api/streams.rs`, `bindings/android/describe.json` … | `src/host_sync.rs`, `src/keybinding_settings.rs`, `src/keybinding_watch.rs` … | no match |
| `kind` | `src/api/agent_sessions.rs`, `src/api/assets.rs`, `src/api/device.rs` … | `src/branch_picker.rs`, `src/browser_cookie_database.rs`, `src/browser_guest.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/NotificationContract.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/Ot3Notifications.kt` … |
| `label` | `src/api/device.rs`, `src/api/orchestration.rs`, `src/api/pull_requests.rs` … | `src/browser_profiles.rs`, `src/connection_access.rs`, `src/connections.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `lastError` | `src/plan_workflow.rs`, `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs`, `src/shell_watch.rs`, `ui/qml/T3UsageRecovery.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `lastErrorClass` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | no match |
| `lastErrorCode` | no match | no match | no match |
| `lastRunOrdinal` | no match | no match | no match |
| `lastVisitedAt` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs`, `ui/qml/T3Backend.qml` | no match |
| `latestLocalTurnOrdinal` | no match | no match | no match |
| `latestRunCompletedAt` | `src/awareness.rs`, `src/sidebar/view.rs`, `src/sidebar.rs` | `src/build_cleanup.rs`, `src/lib.rs`, `ui/qml/T3Backend.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestRunRequestedAt` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestRunStartedAt` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | no match |
| `latestRunStopRequested` | `src/awareness.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `latestUserAuthoredMessageAt` | `src/sidebar/view.rs`, `src/sidebar.rs` | `src/build_cleanup.rs`, `src/lib.rs`, `src/session_memory.rs` | no match |
| `latestUserMessageAt` | `src/sidebar.rs` | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestVersion` | `src/provider_instances.rs` | `src/provider_updates.rs`, `ui/js/ProviderUpdates.js`, `ui/qml/T3Backend.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `latestVisibleMessage` | no match | no match | no match |
| `left` | `src/api/device.rs`, `src/api/pull_requests.rs`, `src/awareness.rs` … | `src/build_cleanup.rs`, `src/device_screen.rs`, `src/host_sync.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/icons/Ot3Icons.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/Ot3Spinner.kt` … |
| `limitRecovery` | `src/api/orchestration.rs` | `src/lib.rs`, `src/nightly.rs`, `ui/qml/T3Backend.qml` … | no match |
| `linkFence` | no match | `src/omabridge_share.rs` | no match |
| `linkedEnvironmentsEnabled` | no match | `src/omabridge_share.rs` | no match |
| `localServerStop` | no match | no match | no match |
| `logsAfterDays` | `src/storage_settings.rs` | `ui/qml/T3StorageSettings.qml` | no match |
| `machine` | `src/api/shell.rs`, `data/i18n.json` | `src/connection_network.rs`, `src/connection_preferences.rs`, `src/connections.rs` … | `app/src/main/assets/licenses/fira-code-OFL.txt`, `app/src/main/assets/licenses/firacode-nerd-OFL.txt`, `app/src/main/assets/licenses/font-awesome.txt` … |
| `matches` | `src/activity.rs`, `src/api/assets.rs`, `src/api/orchestration.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/auto_compact.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/MainActivity.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeScreen.kt` … |
| `maxUploadBytes` | no match | no match | no match |
| `mcpModeLimitHeader` | no match | `src/omabridge_share.rs` | no match |
| `message` | `src/activity.rs`, `src/api/diagnostics.rs`, `src/api/ids.rs` … | `src/auto_compact.rs`, `src/browser_sessions.rs`, `src/build_cleanup.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/connect/ConnectViewModel.kt` … |
| `messageCreatedAt` | `src/api/orchestration.rs` | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/MessageSearch.kt` |
| `messages` | `src/api/orchestration.rs`, `src/checkpoint_revert.rs`, `src/compaction.rs` … | `src/checkpoint_operations.rs`, `src/lib.rs`, `src/live.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/P11_INTEGRATION.md` … |
| `metaKey` | no match | `src/keybinding_settings.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/js/NativeKeybindings.js` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `minutes` | `src/api/scheduled_tasks.rs`, `src/awareness.rs`, `src/compaction.rs` … | `src/auto_compact.rs`, `src/connection_cloud.rs`, `src/idle_release.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/DesktopSurfacesPanel.kt` … |
| `modKey` | no match | `src/keybinding_settings.rs`, `ui/js/NativeKeybindings.js`, `ui/js/ShortcutRouting.js` | no match |
| `mode` | `src/api/diagnostics.rs`, `src/api/mcp_apps.rs`, `src/api/orchestration.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/auto_compact.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/index.json`, `app/src/main/assets/mermaid/mermaid.min.js` … |
| `model` | `src/api/diagnostics.rs`, `src/api/mcp_apps.rs`, `src/api/orchestration.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/auto_compact.rs` … | `app/src/main/assets/licenses/index.json`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `models` | `src/api/diagnostics.rs`, `src/api/orchestration.rs`, `src/api/provider.rs` … | `src/archive_management.rs`, `src/connection_route.rs`, `src/drafts.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/Ot3NavGraph.kt` … |
| `name` | `src/activity.rs`, `src/api/agent_sessions.rs`, `src/api/assets.rs` … | `src/acp_registry.rs`, `src/auto_compact.rs`, `src/backend_service.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `nativeConversationHeadRef` | no match | no match | no match |
| `nativeItemRef` | no match | no match | no match |
| `nativeMetadata` | no match | no match | no match |
| `nativeSessions` | no match | no match | no match |
| `nativeThreadRef` | no match | `src/session_memory.rs` | no match |
| `newProjectsRoot` | no match | `src/write.rs`, `ui/qml/T3AddProject.qml` | no match |
| `newWorktreesStartFromOrigin` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `src/write.rs`, `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `node` | `src/api/ids.rs`, `src/connection_catalog.rs`, `src/markdown.rs` … | `src/backend_service.rs`, `src/browser_runtime.rs`, `src/build_cleanup.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/rendering/P09_VERIFICATION.md`, `core/src/main/jniLibs/arm64-v8a/omabridge-native.properties` … |
| `nodeId` | `src/plan_workflow.rs` | `ui/assets/mermaid/mermaid.min.js` | `app/src/main/assets/mermaid/mermaid.min.js` |
| `nodes` | `src/projection.rs` | `src/browser_runtime.rs`, `src/remote_maintenance_host.rs`, `src/session_memory.rs` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `npmReleaseLookup` | no match | no match | no match |
| `observability` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `ok` | `src/activity.rs`, `src/api/agent_sessions.rs`, `src/api/assets.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/com.squareup.okhttp3.okhttp-4.12.0.txt`, `app/src/main/assets/licenses/com.squareup.okio.okio-jvm-3.9.1.txt` … |
| `opens` | `src/api/ids.rs`, `src/api/mod.rs`, `src/api/orchestration.rs` … | `src/device_screen.rs`, `src/disk_space.rs`, `src/journal_vault.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/font-awesome.txt`, `app/src/main/assets/licenses/mermaid-bundled-notices.txt` … |
| `optionDescriptors` | `src/models.rs` | no match | no match |
| `options` | `src/api/preview.rs`, `src/api/pull_requests.rs`, `src/approvals.rs` … | `src/auto_compact.rs`, `src/browser_runtime.rs`, `src/browser_sources.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/connect/ConnectScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/P11_INTEGRATION.md` … |
| `ordinal` | `src/changes.rs`, `src/checkpoint_revert.rs`, `src/failure_context.rs` … | `src/lib.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/qml/T3Panel.qml` | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/AppearanceSettingsScreen.kt` |
| `otlpEndpointCheck` | no match | no match | no match |
| `otlpLogsUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `otlpMetricsUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `otlpTracesUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `overrides` | `src/domain/mod.rs`, `src/domain/settings.rs`, `src/git_controls.rs` … | `src/connection_route.rs`, `src/lib.rs`, `src/remote_maintenance_host.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/VERIFICATION.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ProjectPresentation.kt` … |
| `ownerNodeId` | no match | no match | no match |
| `parentItemId` | `src/timeline.rs` | `src/lib.rs` | no match |
| `parentNodeId` | no match | no match | no match |
| `peerLinks` | no match | `src/omabridge_share.rs` | no match |
| `pendingBackgroundTasks` | `src/awareness.rs`, `src/sidebar/view.rs`, `src/sidebar.rs` | `src/lib.rs` | no match |
| `pendingGuestMessageCount` | no match | no match | no match |
| `pinOrderKey` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/js/SidebarModel.js`, `ui/qml/T3Backend.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `pinnedAt` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/js/SidebarModel.js`, `ui/qml/T3Backend.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `platform` | `src/api/device.rs`, `src/device_hub.rs`, `src/domain/parity.rs` … | `src/browser_sources.rs`, `src/device_screen.rs`, `src/integration_devices.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraphAccess.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/ThreadFollowService.kt` … |
| `position` | `src/api/assets.rs`, `src/api/pull_requests.rs`, `src/assets.rs` … | `src/browser_sessions.rs`, `src/browser_sources.rs`, `src/connection_network.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/GitSurfaces.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/P11_INTEGRATION.md` … |
| `previousWorktreesDirectories` | no match | no match | no match |
| `profile` | `src/api/ids.rs`, `src/api/preview.rs`, `src/connection_catalog.rs` … | `src/backend_service.rs`, `src/browser_cleanup.rs`, `src/browser_guest.rs` … | `app/src/main/assets/licenses/index.json`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/connections/P13-SDK-GAPS.md` … |
| `projectAgentBrowserAccessOverrides` | no match | no match | no match |
| `projectAutoPullOverrides` | no match | no match | no match |
| `projectCloneTracking` | no match | `src/write.rs` | no match |
| `projectIcon` | `src/api/projects.rs`, `src/projects.rs`, `src/sidebar/view.rs` | `src/archive_management.rs`, `src/lib.rs`, `src/notification_icon.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/Badges.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/KitModels.kt` |
| `projectId` | `src/api/agent_sessions.rs`, `src/api/orchestration.rs`, `src/api/project_clone.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/branch_picker.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeText.kt` … |
| `projectOrganization` | no match | no match | no match |
| `projectProviderInstanceOverrides` | no match | no match | no match |
| `projectScriptOverrides` | `src/git_controls.rs` | no match | no match |
| `projectSettingsFolded` | `src/git_controls.rs`, `src/scoped_settings.rs` | no match | no match |
| `projectSettingsOverrides` | `src/domain/settings.rs`, `src/git_controls.rs`, `src/native_settings.rs` … | `src/connection_route.rs`, `src/scoped_settings.rs`, `src/write.rs` | no match |
| `projectWorktreeCleanup` | `src/scoped_settings.rs`, `src/storage_settings.rs` | `src/scoped_settings.rs`, `ui/qml/T3StorageSettings.qml` | no match |
| `projects` | `src/api/filesystem.rs`, `src/api/mod.rs`, `src/api/orchestration.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/branch_picker.rs` … | `app/src/main/assets/licenses/fira-code-OFL.txt`, `app/src/main/assets/licenses/firacode-nerd-OFL.txt`, `app/src/main/assets/licenses/font-awesome.txt` … |
| `promptInjectedValues` | no match | no match | no match |
| `providerHealthRefreshInterval` | no match | no match | no match |
| `providerInstanceHistory` | `src/sidebar.rs` | no match | no match |
| `providerInstanceId` | `src/api/streams.rs`, `src/api/terminal.rs`, `src/checkpoint_revert.rs` … | `src/checkpoint_operations.rs`, `src/lib.rs`, `src/session_memory.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `providerInstances` | `src/domain/parity.rs`, `src/lib.rs`, `src/models.rs` … | `src/connection_route.rs`, `src/lib.rs`, `src/main.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeParity.kt` |
| `providerModelPreferences` | `src/models.rs` | no match | no match |
| `providerSessionId` | `src/checkpoint_revert.rs`, `src/projection.rs`, `src/queue.rs` | `src/session_memory.rs` | no match |
| `providerThreadId` | `src/checkpoint_revert.rs`, `src/compaction.rs`, `src/domain/review.rs` … | `src/lib.rs`, `src/session_memory.rs`, `ui/qml/T3CheckpointRevert.qml` | no match |
| `providerThreads` | `src/checkpoint_revert.rs`, `src/compaction.rs`, `src/projection.rs` … | `src/lib.rs`, `src/session_memory.rs` | no match |
| `providerTurnId` | `src/domain/review.rs`, `src/tool_inspector.rs`, `bindings/android/describe.json` … | `src/lib.rs`, `ui/qml/T3ToolInspector.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/tools/ToolInspectorRead.kt` |
| `providerWorkspaceSnapshotsOnDemand` | no match | no match | no match |
| `providers` | `src/api/provider.rs`, `src/api/pull_requests.rs`, `src/api/server.rs` … | `src/auto_compact.rs`, `src/checkpoint_operations.rs`, `src/connection_access.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/Ot3NavGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/PreferencesPages.kt` … |
| `pullRequestChecks` | no match | no match | no match |
| `pullRequestMergeMethod` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml`, `ui/qml/T3PullRequests.qml` | no match |
| `pullRequestStackActions` | `src/api/pull_requests.rs` | no match | no match |
| `pullRequests` | `src/api/mod.rs`, `src/api/pull_requests.rs`, `src/domain/git.rs` … | `src/connection_github.rs`, `src/disk_space.rs`, `src/git_operations.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/Badges.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/KitModels.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt` … |
| `questionAttachments` | no match | no match | no match |
| `queuePosition` | `src/queue.rs` | no match | no match |
| `reason` | `src/activity.rs`, `src/api/orchestration.rs`, `src/awareness.rs` … | `src/browser_import.rs`, `src/browser_sources.rs`, `src/document_session.rs` … | `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt`, `app/src/main/assets/licenses/androidx.activity.activity-compose-1.12.4.txt` … |
| `remoteOpenTargets` | no match | no match | no match |
| `removeAgentCreditsOnMerge` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `reportsContextWindow` | no match | no match | no match |
| `repositoryIdentity` | `src/sidebar/view.rs` | `src/connection_route.rs`, `src/lib.rs`, `src/shell_state.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/SidebarView.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/ShellUpdates.kt` |
| `requiredWorktreeBootstrap` | no match | no match | no match |
| `resolvedRepositoryIdentityRoots` | no match | `src/shell_state.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/ShellUpdates.kt` |
| `responseStreamingMode` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `reviewGuideInstructions` | `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `reviewGuideModelSelection` | `src/native_settings.rs`, `src/scoped_settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `reviewGuidesEnabled` | `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `right` | `src/api/device.rs`, `src/api/pull_requests.rs`, `src/diff_review.rs` … | `src/device_screen.rs`, `src/journal_vault.rs`, `src/keybinding_commands.json` … | `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt`, `app/src/main/assets/licenses/androidx.activity.activity-compose-1.12.4.txt` … |
| `role` | `src/checkpoint_revert.rs`, `src/compaction.rs`, `src/device_hub.rs` … | `src/integration_devices.rs`, `src/lib.rs`, `ui/assets/mermaid/mermaid.min.js` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/assets/themes/omarchy-themes.json`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/Ot3Notifications.kt` … |
| `rootNodeId` | `src/plan_workflow.rs` | no match | no match |
| `runId` | `src/activity.rs`, `src/api/orchestration.rs`, `src/changes.rs` … | `src/auto_compact.rs`, `src/branch_picker.rs`, `src/build_cleanup.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadQueue.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` … |
| `runs` | `src/api/orchestration.rs`, `src/api/provider.rs`, `src/api/scheduled_tasks.rs` … | `src/auto_compact.rs`, `src/build_cleanup.rs`, `src/disk_space.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/connect/ConnectScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeViewModel.kt` … |
| `runtimeRequestId` | `src/api/orchestration.rs`, `src/approvals.rs`, `src/domain/mod.rs` … | `src/write.rs`, `ui/qml/T3Backend.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/MessageSender.kt` |
| `schemaVersion` | `src/api/orchestration.rs`, `src/connection_catalog.rs`, `src/settings.rs` | `src/archive_management.rs`, `src/connections.rs`, `src/shell_state.rs` … | no match |
| `scratchWorkspaceRoot` | `src/domain/mod.rs`, `src/models.rs`, `bindings/android/describe.json` … | `src/terminal.rs`, `ui/qml/T3OmaBridgeInstall.qml`, `ui/qml/T3ThreadRow.qml` | no match |
| `scripts` | `src/api/orchestration.rs`, `src/api/projects.rs`, `src/api/worktree_setup.rs` … | `src/agent_sessions.rs`, `src/git_controls.rs`, `src/git_operations.rs` … | `app/src/main/assets/licenses/fira-code-OFL.txt`, `app/src/main/assets/licenses/firacode-nerd-OFL.txt`, `app/src/main/assets/licenses/font-awesome.txt` … |
| `secretProviderSettings` | no match | no match | no match |
| `secretProviders` | no match | no match | no match |
| `secretReferences` | no match | no match | no match |
| `serverBrowser` | no match | `src/browser_guest.rs`, `src/browser_profiles.rs`, `src/browser_sessions.rs` … | no match |
| `serverResolvedCommandContext` | no match | no match | no match |
| `serverUpdateScope` | no match | no match | no match |
| `sessionMethods` | no match | no match | no match |
| `settings` | `src/api/device.rs`, `src/api/diagnostics.rs`, `src/api/projects.rs` … | `src/auto_compact.rs`, `src/browser_address.rs`, `src/browser_guest.rs` … | `app/src/main/assets/mermaid/README.md`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `settledWorktreeCleanup` | no match | no match | no match |
| `setup` | `src/api/mod.rs`, `src/api/provider.rs`, `src/api/streams.rs` … | `src/connection_cloud.rs`, `src/device_watch.rs`, `src/host_sync.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/maintenance/MaintenanceText.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/providers/ProvidersRoute.kt` … |
| `shiftKey` | no match | `src/keybinding_settings.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/js/NativeKeybindings.js` … | `app/src/main/assets/mermaid/mermaid.min.js` |
| `shortcut` | `data/i18n.json` | `src/keybinding_settings.rs`, `ui/assets/mermaid/mermaid.min.js`, `ui/document/T3DocumentWindow.qml` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/PreferencesShell.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/composer/INTEGRATION.md` … |
| `showInteractionModeToggle` | `src/plan_workflow.rs` | `src/write.rs`, `ui/qml/T3Backend.qml` | no match |
| `sideChats` | no match | no match | no match |
| `sidebarAutoSettleAfterDays` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `sidebarAutoSettleKeepPullRequests` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `sidebarAutoSettleOnMerge` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `simlockDeviceHosts` | no match | no match | no match |
| `simlockDeviceHostsEnabled` | no match | no match | no match |
| `skills` | `src/composer.rs`, `src/domain/mod.rs`, `src/i18n.rs` … | `src/host_sync.rs`, `src/remote_maintenance_host.rs`, `src/suggestions.rs` … | no match |
| `slashCommands` | `src/composer.rs` | `src/auto_compact.rs`, `src/suggestions.rs`, `ui/js/NativeUsageLimits.js` | no match |
| `slug` | `src/api/ids.rs`, `src/api/mod.rs`, `src/api/project_clone.rs` … | `src/connection_route.rs`, `src/journal_vault.rs`, `ui/qml/T3ProviderInstances.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md` |
| `snapshotSequence` | `src/api/orchestration.rs`, `src/api/streams.rs`, `src/ffi_stream.rs` … | `src/archive_management.rs`, `src/live.rs`, `src/shell_state.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/connection/ConnectionManager.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` … |
| `snippet` | `src/api/orchestration.rs` | `src/journal_vault.rs`, `src/prompt_stash.rs`, `ui/assets/mermaid/mermaid.min.js` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/MessageSearch.kt` |
| `snoozeLimitedThreads` | `src/native_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `snoozePresets` | no match | no match | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `snoozedAt` | `src/sidebar.rs` | no match | no match |
| `snoozedUntil` | `src/api/orchestration.rs`, `src/awareness.rs`, `src/domain/mod.rs` … | `src/lib.rs`, `src/write.rs`, `ui/qml/T3Panel.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadManagement.kt` |
| `source` | `src/api/assets.rs`, `src/api/diagnostics.rs`, `src/api/mcp_apps.rs` … | `src/browser_address.rs`, `src/browser_cookie_database.rs`, `src/browser_cookies.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `sourceControlHosts` | no match | no match | no match |
| `sourceControlProviders` | `src/api/server.rs` | `src/write.rs` | no match |
| `sourceControlWriterModelSelection` | `src/native_settings.rs`, `src/scoped_settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `sourceControlWritingStyle` | `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `sourceItemId` | `src/domain/review.rs`, `src/history.rs`, `src/tool_inspector.rs` … | `src/failure_context.rs`, `src/lib.rs`, `ui/qml/T3Backend.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ToolInspectorSurface.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/tools/ToolInspectorRead.kt` |
| `sourceThreadId` | `src/api/orchestration.rs`, `src/bounded_snapshot.rs`, `src/domain/review.rs` … | `src/lib.rs`, `src/write.rs`, `ui/qml/T3ToolInspector.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ToolInspectorSurface.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/tools/ToolInspectorRead.kt` |
| `startedAt` | `src/awareness.rs`, `src/domain/presentation.rs`, `src/history.rs` … | `src/checkpoint_journal.rs`, `src/checkpoint_operations.rs`, `src/lib.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarLayout.kt` … |
| `status` | `src/activity.rs`, `src/api/diagnostics.rs`, `src/api/mod.rs` … | `src/auto_compact.rs`, `src/backend_service.rs`, `src/branch_picker.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/assets/themes/omarchy-themes.json`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/NOTICE.md` … |
| `storageCleanup` | `src/api/diagnostics.rs`, `src/api/streams.rs`, `src/native_settings.rs` … | `src/lib.rs`, `src/main.rs`, `src/scoped_settings.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/maintenance/MaintenanceScreen.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/maintenance/P17_GAPS.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/StoragePolicyScreen.kt` … |
| `storageCleanupRun` | `src/native_settings.rs`, `src/storage_settings.rs` | `ui/qml/T3StorageSettings.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt` |
| `streaming` | `src/history.rs`, `src/native_settings.rs`, `src/parity.rs` … | `src/git_operations.rs`, `src/lib.rs`, `ui/qml/T3Backend.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/README.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralTextResources.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadScreen.kt` … |
| `supportedRuntimeModes` | `src/models.rs`, `src/parity.rs` | `ui/qml/T3Backend.qml`, `ui/qml/T3PermissionPicker.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md` |
| `supportsConversationRollback` | `src/checkpoint_revert.rs`, `src/domain/review.rs`, `bindings/android/describe.json` … | `src/checkpoint_operations.rs`, `src/write.rs`, `ui/qml/T3Backend.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` |
| `supportsTextGeneration` | no match | no match | no match |
| `text` | `src/activity.rs`, `src/api/assets.rs`, `src/api/device.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `textGenerationModelSelection` | `src/native_settings.rs`, `src/scoped_settings.rs` | `ui/qml/T3PreferenceField.qml`, `ui/qml/T3Preferences.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralSettingsScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `thread` | `src/api/assets.rs`, `src/api/device.rs`, `src/api/guest_share.rs` … | `src/archive_management.rs`, `src/auto_compact.rs`, `src/backend_service.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `threadActiveReorder` | no match | no match | no match |
| `threadAutoSettleOptOut` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `threadAutoSettlement` | no match | no match | no match |
| `threadAutoSettlementScope` | no match | no match | no match |
| `threadFind` | no match | no match | no match |
| `threadFindProgressive` | no match | no match | no match |
| `threadForceStop` | no match | no match | no match |
| `threadId` | `src/api/assets.rs`, `src/api/device.rs`, `src/api/guest_share.rs` … | `src/archive_management.rs`, `src/auto_compact.rs`, `src/branch_picker.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/NotificationContract.kt` … |
| `threadPinReorder` | `src/api/orchestration.rs` | no match | no match |
| `threadPinning` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `threadProjectFromScratch` | no match | no match | no match |
| `threadPullRequestLinking` | no match | no match | no match |
| `threadPullRequestWatch` | `src/api/orchestration.rs` | no match | no match |
| `threadPullRequests` | no match | no match | no match |
| `threadRestartContinuation` | no match | no match | no match |
| `threadSettlement` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `threadSnooze` | `src/api/orchestration.rs`, `src/models.rs` | `ui/qml/T3ThreadRow.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `threadStop` | no match | no match | no match |
| `threadTitleInstructions` | `src/native_settings.rs`, `src/scoped_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `threadTitleRegeneration` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` |
| `threadVisitedTracking` | no match | no match | no match |
| `threads` | `src/api/orchestration.rs`, `src/api/pull_requests.rs`, `src/awareness.rs` … | `src/archive_management.rs`, `src/auto_compact.rs`, `src/branch_picker.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeRowModel.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/HomeScreen.kt` … |
| `time` | `src/api/device.rs`, `src/api/diagnostics.rs`, `src/api/mod.rs` … | `src/auto_compact.rs`, `src/backend_service.rs`, `src/browser_cleanup.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/androidx.arch.core.core-runtime-2.2.0.txt`, `app/src/main/assets/licenses/androidx.startup.startup-runtime-1.1.1.txt` … |
| `title` | `src/activity.rs`, `src/api/ids.rs`, `src/api/orchestration.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/command_output.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/NotificationContract.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/Ot3Notifications.kt` … |
| `titleRegeneration` | `src/models.rs`, `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3ThreadRow.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/README.md`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ThreadMenuOperations.kt` … |
| `triage` | no match | no match | no match |
| `turnItems` | `src/api/streams.rs`, `src/approvals.rs`, `src/bounded_snapshot.rs` … | `src/failure_context.rs`, `src/lib.rs`, `src/live.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/ThreadCache.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/ThreadSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/FakeT3Transport.kt` … |
| `type` | `src/activity.rs`, `src/api/assets.rs`, `src/api/device.rs` … | `src/archive_management.rs`, `src/auto_compact.rs`, `src/backend_service.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `unavailable` | `src/assets.rs`, `src/checkpoint_revert.rs`, `src/compaction.rs` … | `src/auto_compact.rs`, `src/branch_picker.rs`, `src/browser_cleanup.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/GitSurfaces.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/git/P11_INTEGRATION.md` … |
| `unsettledAt` | `src/sidebar.rs` | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `updateCommand` | no match | `src/provider_updates.rs` | no match |
| `updatedAt` | `src/api/orchestration.rs`, `src/awareness.rs`, `src/compaction.rs` … | `src/auto_compact.rs`, `src/browser_guest.rs`, `src/build_cleanup.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarLayout.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/data/local/Ot3Database.kt` … |
| `usageLimitResetAt` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3ThreadErrorBanner.qml` | no match |
| `usageLimitSources` | `src/api/streams.rs`, `src/domain/usage.rs`, `src/quota_present.rs` … | `src/quota_sources.rs`, `src/quotas.rs`, `ui/js/NativeUsageLimits.js` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/QuotaStrip.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageLimits.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeT3Transport.kt` |
| `usageLimits` | `src/api/streams.rs`, `src/domain/usage.rs`, `src/quota_present.rs` … | `src/local_quotas.rs`, `src/quotas.rs`, `ui/js/NativeUsageLimits.js` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/QuotaStrip.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageLimits.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageScreen.kt` … |
| `usageModelAliases` | no match | no match | no match |
| `usagePriceOverrides` | no match | no match | no match |
| `username` | `src/connection_catalog.rs`, `src/lib.rs`, `src/parity.rs` … | `src/browser_address.rs`, `src/connection_access.rs`, `src/connection_cloud.rs` … | no match |
| `value` | `src/activity.rs`, `src/api/agent_sessions.rs`, `src/api/assets.rs` … | `src/acp_registry.rs`, `src/agent_sessions.rs`, `src/archive_management.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/com.google.auto.value.auto-value-annotations-1.6.3.txt`, `app/src/main/assets/licenses/index.json` … |
| `version` | `src/api/diagnostics.rs`, `src/api/mod.rs`, `src/api/orchestration.rs` … | `src/acp_registry.rs`, `src/archive_management.rs`, `src/browser_cookies.rs` … | `app/src/main/AndroidManifest.xml`, `app/src/main/assets/licenses/Apache-2.0.txt`, `app/src/main/assets/licenses/androidx.activity.activity-1.12.4.txt` … |
| `versionAdvisory` | `src/provider_instances.rs` | `src/provider_updates.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/P07_BINDINGS.md`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `visibility` | `src/bounded_snapshot.rs`, `src/checkpoint_revert.rs`, `src/queue.rs` … | `ui/assets/mermaid/mermaid.min.js`, `ui/js/DeviceSettings.js`, `ui/qml/T3Backend.qml` | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/Ot3Application.kt` … |
| `visibleTurnItems` | `src/bounded_snapshot.rs`, `src/wire.rs` | no match | no match |
| `whenAst` | no match | `src/keybinding_settings.rs`, `ui/js/NativeKeybindings.js`, `ui/qml/T3KeybindingRow.qml` … | no match |
| `windows` | `src/device_hub.rs`, `src/domain/usage.rs`, `src/markdown.rs` … | `src/connections.rs`, `src/device_screen.rs`, `src/journal_vault.rs` … | `app/src/main/assets/mermaid/mermaid.min.js`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/TimelineReader.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/composer/ComposerParity.kt` … |
| `workspaceRoot` | `src/api/agent_sessions.rs`, `src/api/projects.rs`, `src/api/pull_requests.rs` … | `src/agent_sessions.rs`, `src/archive_management.rs`, `src/branch_picker.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/newchat/NewChatProjectPicker.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/maintenance/MaintenanceScreen.kt` … |
| `workspaceSnapshots` | `src/composer.rs` | `src/suggestions.rs`, `ui/js/NativeUsageLimits.js` | no match |
| `worktreeAfterDays` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreeCleanup` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `src/scoped_settings.rs`, `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/StoragePolicyScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt` |
| `worktreeKeepWhen` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/storage_settings.rs` … | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreeOnDelete` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreeOnMerge` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreeSettledAfterDays` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreeSubmodules` | `src/native_settings.rs`, `src/scoped_settings.rs`, `src/settings.rs` … | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `worktreeUnchanged` | `src/scoped_settings.rs`, `src/storage_settings.rs`, `data/storage-rule-cases.json` | `ui/qml/T3StorageSettings.qml`, `data/storage-rule-cases.json` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/ProjectWorktreePolicy.kt` |
| `worktreesDirectory` | `src/storage_settings.rs` | `ui/qml/T3StorageSettings.qml` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/StoragePolicyScreen.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt` |
