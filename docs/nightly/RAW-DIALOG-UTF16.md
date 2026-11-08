# Raw dialog UTF-16 boundary

Reference: nightly4301 (`4cb6945dc60ed03f567dc5daafd083e52308e82b`).

JavaScript JSON strings can contain lone UTF-16 surrogates. A CDP dialog with
such a message is valid JavaScript wire data, but deserializing it directly
into `serde_json::Value` fails before the dialog state can capture it. Preserving
surrogates introduced by a later 4000-unit clip does not address this earlier
transport boundary.

The isolated `cdp_dialog_json` decoder retains only the exact
`Page.javascriptDialogOpening` event's `params.message` and
`params.defaultPrompt` strings as UTF-16 units. Its typed sidecar travels with
the corresponding compatibility `Value`; it must never be recovered through a
later session lookup. The compatibility strings use replacement characters and
are not authoritative text. No reserved property is inserted into incoming JSON.

`RawValue` validates JSON syntax without prematurely converting the selected
strings into Unicode scalar values. An ordered object visitor implements
duplicate keys with the last value and the first insertion position, matching
`JSON.parse`. Other values retain the existing strict scalar boundary. This
does not provide general lone-surrogate support for CDP replies, object keys,
URLs, GUI text, structured MCP content or arbitrary event fields.

The oracle executes native JavaScript `JSON.parse`, `JSON.stringify`, UTF-16
`charCodeAt` and `toWellFormed`, rather than a second custom decoder. Its 140
observations include control characters, escaped backslashes, valid pairs,
isolated high and low surrogates, duplicate fields and replacement of a string
by a non-string. Separate malformed JSON and non-dialog cases preserve the
existing rejection behavior. Tests allocate only in memory and execute through
the private rootless container runner.

Both production readers now carry the typed packet from the inherited NUL pipe
and desktop WebSocket into the same session-fenced dialog observer. Dialog state
clips original units at the native 4,000-unit limit; ordinary response correlation
continues to use the compatibility value. A sidecar cannot be recovered from a
later mutable page lookup or supplied through a reserved untrusted property.

Private acceptance exercises a desktop event through inherited control FDs and
the owned endpoint socket, plus an authenticated provider/MCP interaction with a
held evaluate, an original high and low surrogate in the modal, concurrent status,
answer and continued connection. Exact units are exposed through the structured
inner `content.text` renderer. Raw `structuredContent` explicitly remains a lossy
compatibility projection. These proofs do not establish arbitrary-field Unicode
parity or browser-engine execution of a real JavaScript alert.
