//! Differential driver for complete native control traces; all work is inert.
use o3_server::preview_control as control;
#[path = "support/sandbox.rs"]
mod sandbox;
use control::{Event, Kernel};
use serde_json::{json, Value};
use std::collections::BTreeMap;
struct Driver {
    kernel: Kernel,
    names: BTreeMap<String, u64>,
    outcomes: BTreeMap<String, Value>,
    callbacks: BTreeMap<u64, Value>,
    gates: BTreeMap<String, u64>,
    settled: BTreeMap<String, bool>,
    held: BTreeMap<String, Vec<(u64, u64, Value)>>,
    events: Vec<Value>,
    changes: u64,
    now: f64,
}
impl Driver {
    fn new(agent: Option<String>) -> Self {
        Self {
            kernel: Kernel::new(agent),
            names: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            callbacks: BTreeMap::new(),
            gates: BTreeMap::new(),
            settled: BTreeMap::new(),
            held: BTreeMap::new(),
            events: Vec::new(),
            changes: 0,
            now: 1000.0,
        }
    }
    fn gate(&mut self, name: &str) -> u64 {
        let next = self.gates.len() as u64 + 1;
        *self.gates.entry(name.into()).or_insert(next)
    }
    fn complete_callback(&mut self, action: u64, callback: u64, value: Value, rejected: bool) {
        let operation = &self.callbacks[&callback];
        let result = if rejected {
            Err(json!({"name":"Error","message":"Synthetic tracked work rejected"}))
        } else if operation["fail"] == true {
            Err(json!({"name":"Error","message":"Synthetic action rejected"}))
        } else {
            self.events
                .push(json!({"event":"end","name":operation["name"],"value":value}));
            Ok(value)
        };
        self.kernel.finish_action(action, result);
    }
    fn flush(&mut self) {
        loop {
            let events = self.kernel.drain(self.now);
            if events.is_empty() {
                break;
            }
            for event in events {
                match event {
                    Event::Generation {
                        controller,
                        generation,
                    } => {
                        self.changes += 1;
                        self.events.push(json!({"event":"generation","controller":controller,"generation":generation}));
                    }
                    Event::Complete { action, result } => {
                        let outcome = match result {
                            Ok(value) => json!({"state":"fulfilled","value":value}),
                            Err(error) => json!({"state":"rejected","error":error}),
                        };
                        for (name, _) in self.names.iter().filter(|(_, id)| **id == action) {
                            self.outcomes.insert(name.clone(), outcome.clone());
                        }
                    }
                    Event::Run {
                        action,
                        callback,
                        input,
                    } => {
                        let operation = self.callbacks[&callback].clone();
                        let value = if input.is_null() {
                            operation
                                .get("value")
                                .filter(|value| !value.is_null())
                                .unwrap_or(&operation["name"])
                                .clone()
                        } else {
                            input
                        };
                        self.events
                            .push(json!({"event":"start","name":operation["name"],"value":value}));
                        if let Some(gate) = operation["hold"].as_str() {
                            if let Some(rejected) = self.settled.get(gate).copied() {
                                self.complete_callback(action, callback, value, rejected);
                            } else {
                                self.held
                                    .entry(gate.into())
                                    .or_default()
                                    .push((action, callback, value));
                            }
                        } else {
                            self.complete_callback(action, callback, value, false);
                        }
                    }
                }
            }
        }
    }
    fn apply(&mut self, operation: &Value, callback: u64) -> Value {
        let event_start = self.events.len();
        self.callbacks.insert(callback, operation.clone());
        let name = operation["name"].as_str().unwrap_or("");
        let input = operation
            .get("value")
            .filter(|value| !value.is_null())
            .unwrap_or(&operation["name"])
            .clone();
        let viewer = operation["viewer"].as_str().unwrap_or("");
        let action = match operation["op"].as_str().unwrap() {
            "agent" => Some(self.kernel.agent(
                operation["agent"].as_str().unwrap(),
                callback,
                input,
                operation["deadline"].as_f64(),
            )),
            "human" => Some(self.kernel.human(viewer, callback, input)),
            "system" => Some(self.kernel.system(callback, input)),
            "observe" => Some(self.kernel.observe(callback, input)),
            "latest" => Some(self.kernel.human_latest(
                viewer,
                operation["key"].as_str().unwrap(),
                callback,
                input,
                |waiting, next| {
                    if operation["sum"] == true {
                        json!(waiting.as_i64().unwrap() + next.as_i64().unwrap())
                    } else {
                        next
                    }
                },
            )),
            "take" => Some(self.kernel.take(viewer)),
            "release" | "disconnect" => {
                let after = operation["after"].as_str().map(|name| {
                    let mut callback_operation = operation.clone();
                    callback_operation["name"] = json!(name);
                    self.callbacks.insert(callback, callback_operation);
                    callback
                });
                Some(if operation["op"] == "release" {
                    self.kernel.release(viewer, after)
                } else {
                    self.kernel.disconnect(viewer, after)
                })
            }
            "close" => Some(self.kernel.close()),
            "track" | "handoff" => {
                let name = operation["gate"].as_str().unwrap();
                let gate = self.gate(name);
                if operation["op"] == "track" {
                    self.kernel.track(gate);
                } else {
                    self.kernel.track_until_handoff(gate);
                }
                if self.settled.contains_key(name) {
                    self.kernel.settle_tracked(gate);
                }
                None
            }
            "resolve" | "reject" => {
                let name = operation["gate"].as_str().unwrap();
                let gate = self.gate(name);
                let rejected = operation["op"] == "reject";
                if !self.settled.contains_key(name) {
                    self.settled.insert(name.into(), rejected);
                    self.kernel.settle_tracked(gate);
                    for (action, callback, value) in self.held.remove(name).unwrap_or_default() {
                        self.complete_callback(action, callback, value, rejected);
                    }
                }
                None
            }
            "clock" => {
                self.now = operation["now"].as_f64().unwrap();
                None
            }
            "flush" => None,
            other => panic!("Unknown operation {other}"),
        };
        if let Some(action) = action {
            self.names.insert(name.into(), action);
            self.outcomes
                .insert(name.into(), json!({"state":"pending"}));
        }
        self.flush();
        json!({"agentId":self.kernel.agent_id(),"controller":self.kernel.controller(),"generation":self.kernel.generation(),"changes":self.changes,"canA":self.kernel.agent_can_act_now("A"),"canB":self.kernel.agent_can_act_now("B"),"events":self.events[event_start..],"outcomes":self.outcomes})
    }
}
#[test]
fn complete_native_session_control_traces() {
    sandbox::assert_isolated();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/native-preview-control-4288.json")).unwrap();
    let traces = fixture["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 45);
    for (index, trace) in traces.iter().enumerate() {
        let mut driver = Driver::new(trace["input"]["agentId"].as_str().map(str::to_owned));
        for (step_index, step) in trace["steps"].as_array().unwrap().iter().enumerate() {
            let actual = driver.apply(&step["operation"], step_index as u64 + 1);
            assert_eq!(
                actual, step["expected"],
                "trace {index}, step {step_index}: {}",
                step["operation"]
            );
        }
    }
}
