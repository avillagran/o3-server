# Claude Code 2.1.316 stream-json observations

Captured 2026-10-31 by launching the installed native `claude` executable directly
in temporary `/tmp/o3-claude-*` workspaces. No T3 profile or service was used.
Common argv: `-p --input-format stream-json --output-format stream-json --verbose
--include-partial-messages --permission-prompt-tool stdio --model haiku
--strict-mcp-config --mcp-config '{"mcpServers":{}}'
--settings '{"disableAllHooks":true}'`.

The first stdin frame was
`{"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}`.
User prompts used `type:user`, `message:{role:user,content:...}`,
`parent_tool_use_id:null`, `session_id:""`. Stdin stayed open across results.
The model alias resolved to `claude-haiku-5-5` on this installation.

- `observed.jsonl`: “Reply with exactly OK. Do not use tools.” followed, after
  result, by “Use Bash to run printf probe then reply DONE.” Same process/session.
- `permission.jsonl`: “Use the Write tool to create probe.txt containing OK.”
  Answered the observed `can_use_tool` request using
  `type:control_response,response:{subtype:success,request_id:<observed>,
  response:{behavior:allow,updatedInput:<request.input>}}`. The write succeeded.
- `subagent.jsonl`: requested a foreground general-purpose Haiku Agent returning
  CHILD_OK, followed by PARENT_OK. Includes actual thinking deltas, task_started,
  task_updated, task_notification and tool results. Child text does not belong in
  the parent's assistant message.
- `steer.jsonl`: requested Bash `sleep 2` then FIRST_OK. On the first assistant
  frame, sent another user frame: “Change the final reply to STEER_OK instead.”
  There was one result containing STEER_OK; the process stayed open afterwards.
- `interrupt.jsonl`: same sleep prompt; on the first assistant frame sent
  `{"type":"control_request","request_id":"interrupt-probe","request":{"subtype":"interrupt"}}`.
  Captures success receipt and interrupted/error result (not a successful turn).

Initialization frames are reduced to protocol fields. Initialize responses omit
account, commands, local agents, directories and PID; models/capabilities are kept.
Other frames retain native identifiers and temporary paths to exercise causal
mapping. These are observed frames, not generated success/error fixtures.
The background-boundary test deliberately reorders observed frames to test a race.
The ignored live test additionally proves model/mode controls, PID reuse, Write
permission response, native session resume and explicit child reaping.
