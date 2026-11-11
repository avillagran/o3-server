# Usage limits

Providers publish quota windows in `usageLimits.windows`. Each window is:

| Field | Meaning |
|---|---|
| `id` | Stable window id (`five_hour`, `seven_day`, `primary`, ...). |
| `kind` | `session` or `weekly`. |
| `label` | Display label. |
| `usedPercent` | 0–100. |
| `resetsAt` | ISO timestamp of the next reset, when known. |
| `windowDurationMins` | Length of the window in minutes. |

Clients draw the expected-pace marker on a usage bar from `resetsAt` and
`windowDurationMins`: the marker sits at the fraction of the window already
elapsed. A window without `windowDurationMins` shows no marker.

## Sources

- Codex reports the length itself (`windowDurationMins` from the app server,
  `limit_window_seconds` from the usage API).
- Claude does not report it. Its windows have fixed lengths, set in
  `claude::WINDOWS`: `five_hour` = 300 minutes, `seven_day` = 10080 minutes.
  They are added on all three Claude paths: the OAuth usage poll, the CLI
  `rate_limit` event and the imported usage in `config_sources.rs`. Per-model
  weekly limits use 10080.

Before 48e55e7 the Claude windows lacked `windowDurationMins`, so the desktop and
Android clients showed no pace marker for Claude.
