# Chromium keyboard chords

The agent keyboard path emits the events produced by the exact Playwright1.60.0
dependency in nightly4301 and4314. It handles key names, literal plus tokens,
modifier chords, shifted characters, keypad locations, repeat flags and Chromium
editing commands. It no longer sends a chord such as `Control+v` as one literal
CDP key. Explicit modifiers accept the native Alt/Control/Meta/Shift values.

Each page owns its keyboard state even when pages share a browser process. State
changes precede each transport operation, as in native Keyboard.down/up: if a
later key or send fails, an already-pressed modifier remains pressed. A new page
never inherits another page's state. Browser platform comes from the original
`Browser.getVersion` user agent (Windows wins over Macintosh); ControlOrMeta uses
the server process platform. These two decisions remain separate across hosts.

The generator checks the official package's lock integrity and every retained
file hash, initializes only its original input modules, and executes the full
Keyboard and Chromium RawKeyboard classes. The920 observations cover every
layout entry, four server/browser platform combinations, chords and continued
operations after unknown-key or transport failure. The Rust test compares each
wire field and the resulting modifier/key state. Private real-pipe tests verify
per-page state in a shared process; actual provider/MCP tests verify local and
desktop-channel routing and exact modifier/repeat/shifted events.

Reproduce the original reference only through the private wrapper:

```sh
python3 tools/generate-browser-keyboard-fixtures \
  --artifact /path/to/verified/playwright-core-1.60.0 \
  --node /path/to/node-installation \
  --evidence-root /var/tmp/o3-keyboard-reference
```

The separate retained license and notice preserve the package text (license line
endings are normalized to LF). Release archives
include their full text in THIRD-PARTY-NOTICES.md.

## Remaining browser parity

Nightly exposes these actions through `preview_act(action="press")`. O3 currently
retains its legacy `preview_press` entry point. This change fixes that entry
point's keyboard execution; it does not certify the complete native toolkit,
accessibility locators or action dispatcher. Those remain pending. Playwright's
drag-manager Escape cancellation is also outside this checkpoint because O3 has
no native drag action yet. No general clipboard permission is granted here.

Wire-reference and synthetic-page tests do not establish real page focus,
clipboard contents or end-to-end desktop engine acceptance. Those require the
real GUI/Pool tests and are tracked separately.
