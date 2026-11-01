# P0 field consumer audit

Source: retained nightly 3991 shape report; not a new differential run. Grep is conservative: field names anywhere in client source count as possible reads. No differences are accepted solely on this inventory.

| Field | lib-t3-rust | desktop | Android |
|---|---|---|---|
| `activeAttemptId` | `src/queue.rs` | no match | no match |
| `activeProviderThreadId` | `src/compaction.rs`, `src/checkpoint_revert.rs`, `src/timeline.rs` … | `src/lib.rs`, `src/session_memory.rs` | no match |
| `activityRunStartedAt` | `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `activityRunStatus` | `src/sidebar.rs`, `src/awareness.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `agentActivityPublishing` | no match | no match | no match |
| `attachmentUploads` | no match | no match | no match |
| `attempts` | `src/projection.rs` | `ui/qml/T3ConnectionCloud.qml`, `src/remote_maintenance.rs`, `src/shared_drafts.rs` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/ProviderRefreshGate.kt` |
| `auth` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `src/registry_icon.rs`, `src/quota_present.rs` … | `src/remote_maintenance_host.rs`, `src/remote_maintenance.rs`, `src/remote_surfaces.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalBindings.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt` … |
| `autoSettleDisabledAt` | `src/sidebar/view.rs` | `ui/qml/T3ThreadRow.qml`, `src/lib.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/SidebarView.kt` |
| `blockedOnThreadWait` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `capabilities` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3ConnectionAccess.qml`, `src/remote_surfaces.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/bridge/BridgeCatalogController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt` … |
| `commandOutputStreaming` | no match | no match | no match |
| `connectionProbe` | no match | no match | no match |
| `continuation` | `src/native_settings.rs`, `src/api/orchestration.rs` | no match | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralTextResources.kt` |
| `customInstructions` | `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `databaseMigration` | no match | no match | no match |
| `databaseStorage` | no match | no match | no match |
| `deletedAt` | `src/git_controls.rs`, `src/compaction.rs`, `src/sidebar/view.rs` | `ui/qml/T3Backend.qml`, `src/lib.rs`, `src/session_memory.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` |
| `driver` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/quota_present.rs`, `src/checkpoint_revert.rs` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3Backend.qml`, `ui/qml/T3ProviderUpdateNotice.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/QuotaStrip.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageLimits.kt` … |
| `environment` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `src/quota_present.rs`, `src/lib.rs` … | `src/device_watch.rs`, `src/device_screen.rs`, `src/desktop_open.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` … |
| `environmentIcon` | no match | `src/write.rs`, `ui/qml/T3EnvironmentIcon.qml`, `src/connection_access.rs` … | no match |
| `environmentThemes` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/api/streams.rs` | no match | no match |
| `executionRequested` | no match | no match | no match |
| `fallbackModelSelection` | no match | no match | no match |
| `fileAttachments` | no match | no match | no match |
| `followChangeRequestTemplates` | `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `goal` | `src/theme.rs`, `src/sidebar.rs`, `src/sidebar/view.rs` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3Panel.qml`, `ui/qml/T3Palette.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HomeModels.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/SidebarView.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/KitModels.kt` … |
| `id` | `src/registry_icon.rs`, `src/quota_present.rs`, `src/parity.rs` … | `ui/document/T3QmlPreview.qml`, `ui/style/OmarchyT3/ToolTipPreferences.qml`, `ui/document/T3DocumentWindow.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalEmulator.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalBindings.kt` … |
| `idleCompaction` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/scoped_settings.rs`, `src/native_settings.rs` … | `ui/qml/T3Preferences.qml`, `src/scoped_settings.rs`, `src/native_settings.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `ignoredFileListing` | no match | no match | no match |
| `inlineMessageContext` | no match | no match | no match |
| `instanceId` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/quota_present.rs`, `src/checkpoint_revert.rs` … | `src/main.rs`, `src/shared_editor.rs`, `src/session_memory.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeParity.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` … |
| `item` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt` … | `src/remote_maintenance_host.rs`, `ui/document/T3DocumentWindow.qml`, `src/remote_surfaces.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt` … |
| `keybindings` | `src/api/streams.rs`, `src/api/server.rs` | `src/main.rs`, `ui/qml/T3Backend.qml`, `src/keybinding_watch.rs` … | no match |
| `kind` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `src/quota_present.rs` … | `src/omabridge_route.rs`, `src/remote_maintenance_host.rs`, `src/videos.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/git/GitReviewReader.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/git/GitOperations.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/git/CheckpointRollback.kt` … |
| `lastError` | `src/sidebar.rs`, `src/plan_workflow.rs`, `src/sidebar/view.rs` | `ui/qml/T3UsageRecovery.qml`, `src/lib.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `lastErrorClass` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `lastErrorCode` | no match | no match | no match |
| `lastVisitedAt` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3Backend.qml` | no match |
| `latestLocalTurnOrdinal` | no match | no match | no match |
| `latestRunCompletedAt` | `src/sidebar.rs`, `src/awareness.rs`, `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3Backend.qml`, `src/build_cleanup.rs` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestRunRequestedAt` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/home/layouts/SidebarClocks.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestRunStartedAt` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `latestRunStopRequested` | `src/awareness.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `latestUserAuthoredMessageAt` | `src/sidebar.rs`, `src/sidebar/view.rs` | `src/lib.rs`, `src/session_memory.rs`, `src/build_cleanup.rs` | no match |
| `latestUserMessageAt` | `src/sidebar.rs` | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `latestVisibleMessage` | no match | no match | no match |
| `limitRecovery` | `src/api/orchestration.rs` | `src/lib.rs`, `src/nightly.rs`, `ui/qml/T3UsageRecovery.qml` | no match |
| `linkFence` | no match | `src/omabridge_share.rs` | no match |
| `linkedEnvironmentsEnabled` | no match | `src/omabridge_share.rs` | no match |
| `localServerStop` | no match | no match | no match |
| `machine` | `src/api/shell.rs` | `src/ui_state.rs`, `ui/qml/T3ThreadRow.qml`, `src/omabridge_share.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/ThreadRows.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/kit/KitModels.kt` … |
| `mcpModeLimitHeader` | no match | `src/omabridge_share.rs` | no match |
| `messages` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/compaction.rs` … | `ui/document/T3QmlPreview.qml`, `src/main.rs`, `ui/qml/T3Connections.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HomeStateBuilder.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HomeModels.kt` … |
| `mode` | `src/parity.rs`, `src/usage_summary.rs`, `src/tool_inspector.rs` … | `ui/document/T3QmlPreview.qml`, `ui/qml/T3ContextDivider.qml`, `ui/qml/T3ThreadRow.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalEmulator.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadManagement.kt` … |
| `models` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3ProjectSidebar.qml`, `ui/qml/T3Panel.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadManagement.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/MessageSender.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/newchat/NewChatDraftViewModel.kt` … |
| `nativeSessions` | no match | no match | no match |
| `newProjectsRoot` | no match | `src/write.rs`, `ui/qml/T3AddProject.qml` | no match |
| `nodeId` | `src/plan_workflow.rs` | no match | no match |
| `nodes` | `src/projection.rs` | `src/remote_maintenance_host.rs`, `src/session_memory.rs`, `src/browser_runtime.rs` | no match |
| `npmReleaseLookup` | no match | no match | no match |
| `observability` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `ok` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `src/ffi_stream.rs` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3ConnectionCloud.qml`, `ui/qml/T3CompactionSettings.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/MessageSearch.kt` … |
| `opens` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `src/thread_details.rs`, `src/terminal.rs` … | `ui/qml/T3Panel.qml`, `src/omabridge_route.rs`, `ui/qml/T3Backend.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/git/GitOperations.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/T3Transport.kt` … |
| `options` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `ui/qml/T3ConnectionOmaBridge.qml`, `ui/qml/T3Approvals.qml`, `src/omabridge_route.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/composer/SharedDraftSynchronizer.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ModelPresentation.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/T3Transport.kt` … |
| `otlpEndpointCheck` | no match | no match | no match |
| `otlpLogsUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `otlpMetricsUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `otlpTracesUrl` | `src/native_settings.rs` | `ui/qml/T3Preferences.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` |
| `peerLinks` | no match | `src/omabridge_share.rs` | no match |
| `pendingBackgroundTasks` | `src/sidebar.rs`, `src/awareness.rs`, `src/sidebar/view.rs` | `src/lib.rs` | no match |
| `pendingGuestMessageCount` | no match | no match | no match |
| `pinOrderKey` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3Backend.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt` |
| `pinnedAt` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3Backend.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `platform` | `src/device_hub.rs`, `src/pairing.rs`, `src/domain/parity.rs` … | `src/write.rs`, `ui/qml/T3Backend.qml`, `src/integration_devices.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/Pairing.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/connection/ServerProbe.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/notifications/ThreadFollowService.kt` … |
| `projectCloneTracking` | no match | `src/write.rs` | no match |
| `projectOrganization` | no match | no match | no match |
| `projectProviderInstanceOverrides` | no match | no match | no match |
| `projectSettingsOverrides` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/git_controls.rs`, `src/native_settings.rs` … | `src/write.rs`, `src/scoped_settings.rs`, `src/connection_route.rs` | no match |
| `projectWorktreeCleanup` | `src/storage_settings.rs`, `src/scoped_settings.rs` | `src/scoped_settings.rs`, `ui/qml/T3StorageSettings.qml` | no match |
| `projects` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `src/remote_maintenance_host.rs`, `src/ui_state.rs`, `ui/qml/T3ThreadRow.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/ScopedSettings.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/AppGraph.kt` … |
| `providerInstanceHistory` | `src/sidebar.rs` | no match | no match |
| `providerThreadId` | `src/tool_inspector.rs`, `src/queue.rs`, `src/compaction.rs` … | `ui/qml/T3CheckpointRevert.qml` | no match |
| `providerThreads` | `src/timeline.rs`, `src/queue.rs`, `src/projection.rs` … | `src/lib.rs`, `src/session_memory.rs` | no match |
| `providerWorkspaceSnapshotsOnDemand` | no match | no match | no match |
| `providers` | `src/quota_present.rs`, `src/parity.rs`, `src/quota_sources.rs` … | `src/ui_state.rs`, `ui/qml/T3CompactionSettings.qml`, `ui/qml/T3ThreadRow.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ModelPresentation.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeParity.kt` … |
| `pullRequestChecks` | no match | no match | no match |
| `pullRequestStackActions` | `src/api/pull_requests.rs` | no match | no match |
| `pullRequests` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/sidebar.rs` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3Panel.qml`, `src/disk_space.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/pullrequests/PullRequestWrites.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/pullrequests/PullRequestReader.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/pullrequests/PullRequestsViewModel.kt` |
| `questionAttachments` | no match | no match | no match |
| `queuePosition` | `src/queue.rs` | no match | no match |
| `remoteOpenTargets` | no match | no match | no match |
| `reportsContextWindow` | no match | no match | no match |
| `requiredWorktreeBootstrap` | no match | no match | no match |
| `resolvedRepositoryIdentityRoots` | no match | `src/shell_state.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/ShellUpdates.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `role` | `src/parity.rs`, `src/history.rs`, `src/timeline.rs` … | `src/lib.rs`, `ui/qml/T3ThreadRow.qml`, `ui/qml/T3ContextMeter.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/notifications/Ot3Notifications.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/theme/ThemeTokens.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HomeStateBuilder.kt` … |
| `rootNodeId` | `src/plan_workflow.rs` | no match | no match |
| `runs` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `src/history.rs`, `src/ffi_call.rs` … | `src/remote_maintenance_host.rs`, `src/main.rs`, `src/lib.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeT3Transport.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/SidebarView.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/pairing/PairingService.kt` … |
| `scratchWorkspaceRoot` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/models.rs`, `src/domain/mod.rs` | `ui/qml/T3ThreadRow.qml`, `src/terminal.rs`, `ui/qml/T3OmaBridgeInstall.qml` | no match |
| `secretProviderSettings` | no match | no match | no match |
| `secretProviders` | no match | no match | no match |
| `secretReferences` | no match | no match | no match |
| `serverBrowser` | no match | `ui/qml/T3Backend.qml`, `src/preview_transport.rs`, `src/browser_profiles.rs` … | no match |
| `serverResolvedCommandContext` | no match | no match | no match |
| `serverUpdateScope` | no match | no match | no match |
| `sessionMethods` | no match | no match | no match |
| `settings` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/lib.rs` … | `ui/style/OmarchyT3/ToolTipPreferences.qml`, `ui/qml/T3ThreadRow.qml`, `ui/qml/T3Panel.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/pairing/PairingService.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` … |
| `settledWorktreeCleanup` | no match | no match | no match |
| `setup` | `src/stream.rs`, `src/latex.rs`, `src/provider_instances.rs` … | `src/omabridge_route.rs`, `src/main.rs`, `src/lib.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/bridge/NativeBridge.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/maintenance/MaintenanceText.kt` … |
| `showInteractionModeToggle` | `src/plan_workflow.rs` | `src/write.rs`, `ui/qml/T3Backend.qml` | no match |
| `sideChats` | no match | no match | no match |
| `simlockDeviceHosts` | no match | no match | no match |
| `simlockDeviceHostsEnabled` | no match | no match | no match |
| `skills` | `src/composer.rs`, `src/i18n.rs`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `src/remote_maintenance_host.rs`, `src/suggestions.rs`, `ui/qml/T3Composer.qml` … | no match |
| `slashCommands` | `src/composer.rs` | `src/suggestions.rs`, `src/auto_compact.rs` | no match |
| `snoozePresets` | no match | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt` |
| `snoozedAt` | `src/sidebar.rs` | no match | no match |
| `snoozedUntil` | `src/sidebar.rs`, `src/awareness.rs`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `src/lib.rs`, `ui/qml/T3ThreadRow.qml`, `src/write.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadManagement.kt` |
| `sourceControlWritingStyle` | `src/scoped_settings.rs`, `src/settings.rs` | `ui/qml/T3Preferences.qml` | no match |
| `status` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `ui/qml/T3ThreadRow.qml`, `ui/qml/T3Panel.qml`, `src/omabridge_route.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/HomeStateBuilder.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/git/GitOperations.kt` … |
| `storageCleanup` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/scoped_settings.rs`, `src/storage_settings.rs` … | `ui/qml/T3Backend.qml`, `ui/qml/T3StorageSettings.qml`, `src/scoped_settings.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/StoragePolicyScreen.kt` |
| `storageCleanupRun` | `src/storage_settings.rs`, `src/native_settings.rs` | `ui/qml/T3StorageSettings.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt` |
| `streaming` | `src/parity.rs`, `src/timeline.rs`, `src/history.rs` … | `ui/qml/T3Panel.qml`, `ui/qml/T3Backend.qml`, `src/lib.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/thread/ThreadTimeline.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/TimelineReader.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralTextResources.kt` … |
| `supportedRuntimeModes` | `src/parity.rs`, `src/models.rs` | `ui/qml/T3Backend.qml`, `ui/qml/T3PermissionPicker.qml` | no match |
| `supportsConversationRollback` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/checkpoint_revert.rs`, `src/domain/review.rs` | `src/write.rs`, `ui/qml/T3Panel.qml`, `ui/qml/T3Backend.qml` … | `app/src/main/kotlin/dev/avillagran/ot3android/ui/thread/ThreadViewModel.kt` |
| `text` | `src/registry_icon.rs`, `src/quota_present.rs`, `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt` … | `ui/document/T3QmlPreview.qml`, `ui/style/OmarchyT3/ToolTipPreferences.qml`, `ui/style/OmarchyT3/ToolTip.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalEmulator.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` … |
| `textGenerationModelSelection` | `src/scoped_settings.rs`, `src/native_settings.rs` | `ui/qml/T3Preferences.qml`, `ui/qml/T3PreferenceField.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/general/GeneralSettingsScreen.kt` |
| `thread` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt` … | `src/omabridge_route.rs`, `ui/qml/T3CompactionSettings.qml`, `ui/qml/T3CommandPalette.qml` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalBindings.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/settings/GeneralSettingsFields.kt` … |
| `threadActiveReorder` | no match | no match | no match |
| `threadAutoSettleOptOut` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | no match |
| `threadAutoSettlement` | no match | no match | no match |
| `threadAutoSettlementScope` | no match | no match | no match |
| `threadFind` | no match | no match | no match |
| `threadFindProgressive` | no match | no match | no match |
| `threadForceStop` | no match | no match | no match |
| `threadPinReorder` | no match | no match | no match |
| `threadPinning` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | no match |
| `threadProjectFromScratch` | no match | no match | no match |
| `threadPullRequestLinking` | no match | no match | no match |
| `threadPullRequestWatch` | no match | no match | no match |
| `threadPullRequests` | no match | no match | no match |
| `threadRestartContinuation` | no match | no match | no match |
| `threadSettlement` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | no match |
| `threadSnooze` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | no match |
| `threadStop` | no match | no match | no match |
| `threadTitleRegeneration` | `src/models.rs` | `ui/qml/T3ThreadRow.qml` | no match |
| `threadVisitedTracking` | no match | no match | no match |
| `threads` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/lib.rs` … | `src/remote_maintenance_host.rs`, `src/ui_state.rs`, `src/main.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/MaintenanceController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/home/ThreadActions.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/git/GitTarget.kt` … |
| `titleRegeneration` | `src/sidebar/view.rs` | `ui/qml/T3ThreadRow.qml`, `src/lib.rs` | `app/src/main/kotlin/dev/avillagran/ot3android/ui/navigation/AppScreens.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/projects/ThreadMenuOperations.kt` |
| `triage` | no match | no match | no match |
| `turnItems` | `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt`, `src/history.rs`, `src/bounded_snapshot.rs` … | `src/remote_maintenance_host.rs`, `src/lib.rs`, `src/session_memory.rs` … | no match |
| `unsettledAt` | `src/sidebar.rs` | no match | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt` |
| `updatedAt` | `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt`, `src/parity.rs`, `src/history.rs` … | `ui/qml/T3Panel.qml`, `src/main.rs`, `src/lib.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/shell/ShellReducer.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/data/local/OutboxEntity.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/data/local/Ot3Database.kt` … |
| `usageLimitResetAt` | `src/sidebar/view.rs` | `src/lib.rs`, `ui/qml/T3ThreadErrorBanner.qml` | no match |
| `usageLimitSources` | `src/quota_present.rs`, `src/quota_sources.rs`, `bindings/android/kotlin/dev/avillagran/t3/T3Api.kt` … | `src/quotas.rs`, `src/quota_sources.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/transport/NativeT3Transport.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/QuotaStrip.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/usage/UsageLimits.kt` |
| `usageModelAliases` | no match | no match | no match |
| `usagePriceOverrides` | no match | no match | no match |
| `value` | `bindings/android/tests/dev/avillagran/t3/StreamFixture.kt`, `bindings/android/kotlin/dev/avillagran/t3/T3Client.kt`, `bindings/android/tests/dev/avillagran/t3/KotlinFixture.kt` … | `src/omabridge_route.rs`, `ui/qml/T3ConnectionOmaBridge.qml`, `src/remote_maintenance_host.rs` … | `core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt`, `core/src/main/kotlin/dev/avillagran/ot3android/core/bridge/BridgePairingReview.kt` … |
| `versionAdvisory` | `src/provider_instances.rs` | `src/provider_updates.rs` | `core/src/main/kotlin/dev/avillagran/ot3android/core/providers/ProviderAdministration.kt` |
| `visibleTurnItems` | `src/wire.rs`, `src/bounded_snapshot.rs` | no match | no match |
| `worktreesDirectory` | `src/storage_settings.rs` | `ui/qml/T3StorageSettings.qml` | `core/src/main/kotlin/dev/avillagran/ot3android/core/maintenance/StoragePolicyController.kt`, `app/src/main/kotlin/dev/avillagran/ot3android/ui/settings/storage/StoragePolicyScreen.kt` |
