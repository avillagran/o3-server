// Driver for tools/qml-e2e: the real O3 desktop panel and real backend worker, headless.
// Steps print QML_E2E_STEP; success QML_E2E_PASS; any failure QML_E2E_FAIL (exit 1).
import QtQuick
import Quickshell
import QtTest
import "UnderTest/ui/qml" as Test

ShellRoot {
  id: root
  property int phase: 0
  property int ticks: 0
  property int phaseTicks: 0
  property string threadId: ""
  readonly property string title: Quickshell.env("QML_E2E_TITLE")
  readonly property string seedWord: Quickshell.env("QML_E2E_SEED_WORD")
  readonly property string pingWord: Quickshell.env("QML_E2E_PING_WORD")
  readonly property var backend: Test.T3Backend

  function step(m) { console.log("QML_E2E_STEP " + m) }
  function fail(m) {
    console.error("QML_E2E_FAIL " + m)
    shot("fail-phase" + phase)
    failExit.start()
  }
  function check(ok, m) { if (ok) step(m); else fail(m); return ok }
  Timer { id: failExit; interval: 1500; onTriggered: Qt.exit(1) }

  // The window's QML content: the child of the window's content item.
  function windowItem() {
    var item = panel.composerView
    while (item.parent && !/ProxyWindowContentItem|RootItem/.test(String(item.parent))) item = item.parent
    return item
  }
  // The runner captures the compositor output (grim) when it sees this line; hold a few ticks for it.
  property int hold: 0
  property bool idleShot: false
  function shot(name) { console.log("QML_E2E_SHOT " + name); hold = 6 }
  // A visible item whose text contains `needle`, anywhere under `item`.
  function find(item, needle, exclude) {
    if (!item || item.visible === false) return null
    if (typeof item.text === "string" && item.text.indexOf(needle) >= 0 && !(exclude && item.text.indexOf(exclude) >= 0)) return item
    var kids = item.children || []
    for (var i = 0; i < kids.length; i++) { var f = find(kids[i], needle, exclude); if (f) return f }
    return null
  }
  // The transcript holds a finished assistant message containing `word` (the prompt is a user message).
  function replied(word) {
    return (backend.transcript || []).some(function(m) { return m.role === "assistant" && !m.streaming && (m.text || "").indexOf(word) >= 0 })
  }
  function users(word) { return (backend.transcript || []).filter(function(m) { return m.role === "user" && (m.text || "").indexOf(word) >= 0 }).length }
  function seedThread() {
    var list = backend.state.threads || []
    for (var i = 0; i < list.length; i++) if (list[i].title === title) return list[i]
    return null
  }
  function write(value) {
    backend.patchDraft(backend.selectedKey, { text: value, attempt: null, busy: false, error: "" })
    panel.composerView.inputField.forceActiveFocus()
    Qt.callLater(function() { panel.composerView.inputField.cursorPosition = value.length })
  }

  TestCase { id: keys; when: false; parent: panel.composerView.inputField }
  Test.T3Panel { id: panel; bridge: Test.T3Backend; standalone: true }
  // As app/shell.qml does for the real app.
  Connections { target: panel; function onOpenedChanged() { Test.T3Backend.setPanelVisible(panel.opened) } }

  Timer {
    interval: 250; repeat: true; running: true
    onTriggered: {
      if (root.hold > 0) { root.hold--; return }
      ticks++; phaseTicks++
      var b = root.backend
      if (phase === 0) {
        // The backend discovered the isolated server through PATH `t3` and T3CODE_HOME.
        if (phaseTicks > 240) return root.fail("no ready local environment with the seeded thread (state: " + JSON.stringify(b.state.environments) + ")")
        var t = root.seedThread()
        if (!b.viewStateLoaded || !t) return
        root.threadId = t.id
        if (!check((b.state.environments || []).some(function(e) { return e.id === "local" && e.state === "ready" }), "local environment ready")) return
        panel.open(); phase = 1; phaseTicks = 0
      } else if (phase === 1) {
        if (phaseTicks < 8) return
        if (phaseTicks > 40) return root.fail("sidebar never rendered the thread title")
        if (!root.find(root.windowItem(), root.title)) return
        root.step("sidebar shows thread '" + root.title + "'")
        root.shot("01-sidebar"); phase = 2; phaseTicks = 0
      } else if (phase === 2) {
        if (phaseTicks === 1) b.selectThread("local", root.threadId)
        if (phaseTicks > 80) return root.fail("opened thread never loaded the seeded reply (transcript: " + JSON.stringify(b.transcript).slice(0, 400) + ")")
        if (!root.replied(root.seedWord) || root.users(root.seedWord) < 1) return
        if (phaseTicks < 12) return
        // Both bubbles are in the scene: the prompt, and the assistant text that is not the prompt.
        if (!check(root.find(root.windowItem(), "Reply with exactly the word " + root.seedWord) !== null
                   && root.find(root.windowItem(), root.seedWord, "Reply with") !== null, "opened thread renders the seeded user and assistant messages")) return
        root.shot("02-thread"); phase = 3; phaseTicks = 0
      } else if (phase === 3) {
        if (b.draft.loading) return
        var sent = "Reply with exactly the word " + root.pingWord + " and nothing else."
        root.write(sent); phase = 4; phaseTicks = 0
      } else if (phase === 4) {
        if (phaseTicks < 3) return
        keys.keyClick(Qt.Key_Return)
        if (!check(root.users(root.pingWord) > 0 || !!(b.draft.attempt || b.draft.busy), "composer Enter submitted the message")) return
        phase = 5; phaseTicks = 0
      } else if (phase === 5) {
        if (phaseTicks > 480) return root.fail("no reply to the composer message within 120 s (transcript: " + JSON.stringify(b.transcript).slice(-500) + ")")
        if (!root.replied(root.pingWord)) return
        if (root.users(root.pingWord) !== 1) return root.fail("the composer prompt did not appear exactly once as a user message")
        if (root.find(root.windowItem(), root.pingWord, "Reply with") === null) return
        root.step("assistant reply to the composer message renders")
        root.shot("03-reply"); phase = 6; phaseTicks = 0
      } else if (phase === 6) {
        var t = root.seedThread() || {}
        if ((t.status !== "completed" || t.working) && phaseTicks < 80) return
        if (!root.idleShot) { root.idleShot = true; root.shot("04-idle"); return }
        console.log("QML_E2E_STEP final thread state: status=" + t.status + " working=" + t.working + " pending=" + t.pending + " reason=" + t.reason + " settled=" + t.settled)
        console.log("QML_E2E_PASS")
        Qt.quit()
      }
    }
  }
}
