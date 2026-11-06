mod support;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_protocol::Connection;
const WAIT: Duration = Duration::from_secs(12);
fn projection(c: &Connection, t: &str) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":t}))
        .unwrap()
}
fn command(c: &Connection, v: Value) {
    c.rpc("orchestration.dispatchCommand", v).unwrap();
}
fn frames(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("selection-trace.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
fn native_pid(root: &Path) -> u32 {
    frames(root)
        .iter()
        .rev()
        .find(|f| matches!(f["method"].as_str(), Some("turn/start" | "claude/user")))
        .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32
}
fn alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}
struct Fixture {
    s: support::Server,
    c: Connection,
    root: PathBuf,
    peer_root: PathBuf,
    entries: Value,
}
impl Fixture {
    fn new(driver: &str) -> Self {
        let s = support::start(|s| {
            *s.provider.lock().unwrap() =
                Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    s.dir.clone(),
                )))
        });
        let c = s.connect();
        let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selection-peer.py");
        let entries = json!({"work":{"driver":driver,"enabled":true,"config":{"binaryPath":binary,"homePath":s.dir.join("old-home")}},"peer":{"driver":driver,"enabled":true,"config":{"binaryPath":binary,"homePath":s.dir.join("peer-home")}}});
        c.rpc(
            "server.updateSettings",
            json!({"patch":{"providerInstances":entries}}),
        )
        .unwrap();
        for id in ["work", "peer"] {
            let root = s.dir.join(id);
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join(".selection-fixture"), "").unwrap();
            c.rpc("projects.mutate",json!({"type":"project.create","commandId":format!("p-{id}"),"projectId":id,"title":id,"workspaceRoot":root})).unwrap();
            command(
                &c,
                json!({"type":"thread.create","commandId":format!("create-{id}"),"threadId":id,"projectId":id,"title":id,"modelSelection":{"instanceId":id,"model":"one"},"runtimeMode":"full-access","interactionMode":"default"}),
            );
        }
        let root = s.dir.join("work");
        let peer_root = s.dir.join("peer");
        Self {
            s,
            c,
            root,
            peer_root,
            entries,
        }
    }
    fn send(&self, t: &str, id: &str, text: &str) {
        command(
            &self.c,
            json!({"type":"message.dispatch","commandId":id,"threadId":t,"messageId":id,"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
    }
    fn done(&self, t: &str) {
        assert!(support::wait_for(WAIT, || projection(&self.c, t)["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "completed")
            && self.s.state.active.lock().unwrap().is_empty()));
    }
    fn settings(&self) {
        self.c
            .rpc(
                "server.updateSettings",
                json!({"patch":{"providerInstances":self.entries}}),
            )
            .unwrap();
    }
    fn cleanup(&self) {
        for t in ["work", "peer"] {
            command(
                &self.c,
                json!({"type":"thread.delete","commandId":format!("delete-{t}"),"threadId":t}),
            );
        }
    }
}
#[test]
fn replacing_and_removing_both_native_pools_reaps_old_processes_and_preserves_peer_bindings() {
    for driver in ["codex", "claudeAgent"] {
        let mut f = Fixture::new(driver);
        f.send("work", "first", "NEXT old");
        f.done("work");
        f.send("peer", "sibling", "NEXT peer");
        f.done("peer");
        let old = native_pid(&f.root);
        let peer = native_pid(&f.peer_root);
        f.settings();
        assert!(alive(old));
        assert!(alive(peer));
        std::fs::write(f.root.join(".selection-previous-pid"), old.to_string()).unwrap();
        f.entries["work"]["config"]["homePath"] = json!(f.s.dir.join("new-home"));
        f.settings();
        f.send("work", "second", "NEXT replacement");
        f.done("work");
        let new = native_pid(&f.root);
        assert_ne!(new, old);
        assert!(!alive(old));
        assert!(alive(peer));
        let incoming = frames(&f.root)
            .into_iter()
            .find(|v| {
                v["pid"] == new
                    && matches!(v["method"].as_str(), Some("turn/start" | "claude/user"))
            })
            .unwrap();
        assert_eq!(
            incoming["previousAlive"], false,
            "replacement input reached its process before the old scope was reaped"
        );
        f.entries.as_object_mut().unwrap().remove("work");
        f.settings();
        assert!(support::wait_for(WAIT, || !alive(new)));
        assert!(alive(peer));
        f.send("peer", "peer-again", "NEXT retained");
        f.done("peer");
        assert_eq!(native_pid(&f.peer_root), peer);
        f.cleanup();
    }
}
#[test]
fn settings_replacement_cancels_inflight_native_work_before_admitting_the_new_pool() {
    for driver in ["codex", "claudeAgent"] {
        let mut f = Fixture::new(driver);
        f.send("work", "held", "HOLD original");
        assert!(support::wait_for(WAIT, || projection(&f.c, "work")
            ["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["status"] == "running")));
        let old = native_pid(&f.root);
        std::fs::write(f.root.join(".selection-previous-pid"), old.to_string()).unwrap();
        f.entries["work"]["config"]["homePath"] = json!(f.s.dir.join("new-home"));
        f.settings();
        assert!(support::wait_for(WAIT, || !alive(old)
            && f.s.state.active.lock().unwrap().is_empty()));
        let p = projection(&f.c, "work");
        assert!(matches!(
            p["runs"][0]["status"].as_str(),
            Some("failed" | "interrupted")
        ));
        f.send("work", "replacement", "NEXT replacement");
        assert!(support::wait_for(WAIT, || projection(&f.c, "work")["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |r| r["userMessageId"] == "replacement" && r["status"] == "completed"
            )));
        let new = native_pid(&f.root);
        assert_ne!(new, old);
        assert!(frames(&f.root)
            .iter()
            .filter(|v| v["pid"] == new)
            .all(|v| v["previousAlive"] == false));
        f.cleanup();
    }
}

fn marked_native_pids(marker: &str) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|p| {
            let p = p.ok()?;
            let pid = p.file_name().to_str()?.parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(p.path().join("stat")).ok()?;
            let fields = stat[stat.rfind(')')? + 1..]
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.get(1)?.parse::<u32>().ok()? != std::process::id() {
                return None;
            }
            let env = std::fs::read(p.path().join("environ")).ok()?;
            env.split(|b| *b == 0)
                .any(|v| v == format!("O3_SCOPE_PROOF={marker}").as_bytes())
                .then_some(pid)
        })
        .collect()
}
#[test]
#[ignore = "installed Codex/Claude shells, registry replacement, owned process reaping and native-history continuation"]
fn live_scope_replacement_reaps_active_shells_and_resumes_private_history() {
    for driver in ["codex", "claudeAgent"] {
        let s = support::start(|s| {
            *s.provider.lock().unwrap() =
                Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    s.dir.clone(),
                )))
        });
        let c = s.connect();
        let root = s.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        let home = std::env::var(if driver == "codex" {
            "CODEX_HOME"
        } else {
            "CLAUDE_CONFIG_DIR"
        })
        .expect("run with tools/live-transfer-e2e");
        let binary = std::env::var(if driver == "codex" {
            "O3_LIVE_CODEX_BINARY"
        } else {
            "O3_LIVE_CLAUDE_BINARY"
        })
        .unwrap_or_else(|_| if driver == "codex" { "codex" } else { "claude" }.into());
        let mut entry = json!({"driver":driver,"enabled":true,"config":{"homePath":home,"binaryPath":binary},"environment":[{"name":"O3_SCOPE_PROOF","value":format!("old-{driver}")}]});
        c.rpc(
            "server.updateSettings",
            json!({"patch":{"providerInstances":{"work":entry}}}),
        )
        .unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Scope replacement","workspaceRoot":root})).unwrap();
        let model = if driver == "codex" {
            "gpt-6.1-sol"
        } else {
            "haiku"
        };
        command(
            &c,
            json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Scope replacement","modelSelection":{"instanceId":"work","model":model,"options":if driver=="codex"{json!([{"id":"reasoningEffort","value":"low"}])}else{json!([])}},"runtimeMode":"full-access","interactionMode":"default"}),
        );
        let word = format!("scope-{}", o3_server::store::random_id().unwrap());
        command(
            &c,
            json!({"type":"message.dispatch","commandId":"first","threadId":"t","messageId":"first","text":format!("Remember private codeword {word}. Execute exactly one shell command in this workspace: echo $$ > .scope-shell.pid; sleep 12; printf 'SHELL-FINISHED'. Then reply only with the codeword. Use no other tools."),"attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(Duration::from_secs(90), || {
            let p = projection(&c, "t");
            assert_ne!(
                p["runs"][0]["status"], "failed",
                "{}",
                p["runs"][0]["error"]
            );
            std::fs::read_to_string(root.join(".scope-shell.pid"))
                .ok()
                .is_some_and(|p| p.trim().parse::<u32>().is_ok())
        }));
        let shell = std::fs::read_to_string(root.join(".scope-shell.pid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        let native = marked_native_pids(&format!("old-{driver}"));
        assert_eq!(native.len(), 1);
        let before = projection(&c, "t");
        let native_ref = before["providerThreads"][0]["nativeThreadRef"].clone();
        entry["environment"][0]["value"] = json!(format!("new-{driver}"));
        c.rpc(
            "server.updateSettings",
            json!({"patch":{"providerInstances":{"work":entry}}}),
        )
        .unwrap();
        assert!(
            support::wait_for(Duration::from_secs(30), || !alive(shell)
                && !alive(native[0])
                && s.state.active.lock().unwrap().is_empty()),
            "{driver}: old native scope left a live CLI or shell"
        );
        command(
            &c,
            json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"Reply only with the private codeword from my earlier user message. Execute no tools.","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(Duration::from_secs(90), || {
            let p = projection(&c, "t");
            let r = p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["userMessageId"] == "next")
                .unwrap();
            assert_ne!(r["status"], "failed", "{driver}: {}", r["error"]);
            r["status"] == "completed"
        }));
        let p = projection(&c, "t");
        let run = p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["userMessageId"] == "next")
            .unwrap();
        assert_eq!(p["providerThreads"][0]["nativeThreadRef"], native_ref);
        assert!(p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "assistant"
                && m["runId"] == run["id"]
                && m["text"].as_str().is_some_and(|text| text.contains(&word))));
        command(
            &c,
            json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
        );
        println!("Installed {driver}: changed scope reaped active shell and CLI before replacement; fresh process resumed private native history");
    }
}

#[test]
fn explicit_builtin_override_and_removal_close_only_the_replaced_builtin_scope() {
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selection-peer.py");
    let s = support::start(|s| {
        let defaults = Arc::new(o3_server::provider::Providers {
            codex: Arc::new(o3_server::codex::CodexProvider::new(
                binary.clone(),
                Some(s.dir.join("default-home")),
            )),
            claude: Some(Arc::new(
                o3_server::claude::ClaudeProvider::new(binary.clone()).with_environment(vec![(
                    "CLAUDE_CONFIG_DIR".into(),
                    s.dir.join("default-claude").to_string_lossy().into_owned(),
                )]),
            )),
        });
        *s.provider.lock().unwrap() = Some(Arc::new(
            o3_server::provider::ConfiguredProviders::new(defaults, s.dir.clone()),
        ));
    });
    let c = s.connect();
    for (t, instance) in [("work", "codex"), ("peer", "claudeAgent")] {
        let root = s.dir.join(t);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".selection-fixture"), "").unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":format!("p-{t}"),"projectId":t,"title":t,"workspaceRoot":root})).unwrap();
        command(
            &c,
            json!({"type":"thread.create","commandId":format!("create-{t}"),"threadId":t,"projectId":t,"title":t,"modelSelection":{"instanceId":instance,"model":"one"},"runtimeMode":"full-access","interactionMode":"default"}),
        );
        command(
            &c,
            json!({"type":"message.dispatch","commandId":format!("first-{t}"),"threadId":t,"messageId":format!("first-{t}"),"text":"NEXT first","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(WAIT, || projection(&c, t)["runs"][0]
            ["status"]
            == "completed"
            && s.state.active.lock().unwrap().is_empty()));
    }
    let root = s.dir.join("work");
    let peer = native_pid(&s.dir.join("peer"));
    let mut previous = native_pid(&root);
    for (cid, entries) in [
        (
            "override",
            json!({"codex":{"driver":"codex","enabled":true,"config":{"binaryPath":binary,"homePath":s.dir.join("override-home")}}}),
        ),
        ("fallback", json!({})),
    ] {
        std::fs::write(root.join(".selection-previous-pid"), previous.to_string()).unwrap();
        c.rpc(
            "server.updateSettings",
            json!({"patch":{"providerInstances":entries}}),
        )
        .unwrap();
        command(
            &c,
            json!({"type":"message.dispatch","commandId":cid,"threadId":"work","messageId":cid,"text":"NEXT replacement","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(WAIT, || projection(&c, "work")["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |r| r["userMessageId"] == cid && r["status"] == "completed"
            )
            && s.state.active.lock().unwrap().is_empty()));
        let current = native_pid(&root);
        assert_ne!(current, previous);
        assert!(!alive(previous));
        assert!(alive(peer));
        assert!(frames(&root)
            .iter()
            .filter(|v| v["pid"] == current)
            .all(|v| v["previousAlive"] == false));
        previous = current;
    }
    for t in ["work", "peer"] {
        command(
            &c,
            json!({"type":"thread.delete","commandId":format!("delete-{t}"),"threadId":t}),
        );
    }
}
