//! Codex goal controls never become slash-command inference prompts.
use super::{Client, ProviderEvent};
use crate::native_goal::{self, Command};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub(super) fn rejection(error: String) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(&error) {
        if let Some(message) = value["message"].as_str() {
            static VALIDATION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            if VALIDATION
                .get_or_init(|| {
                    regex::Regex::new(
                        r"^goal objective must (?:not be empty|be at most [0-9]{1,6} characters)$",
                    )
                    .unwrap()
                })
                .is_match(message)
            {
                return format!("Codex rejected the goal: {message}.");
            }
        }
    }
    error
}

pub(super) struct State {
    pub current: Value,
    pub managed: bool,
    pub origin: &'static str,
    activation_observed: bool,
    activation_updated: bool,
}

impl State {
    pub fn new(current: Value) -> Self {
        Self {
            current,
            managed: false,
            origin: "automatic",
            activation_observed: false,
            activation_updated: false,
        }
    }

    pub fn active(&self) -> bool {
        self.current["status"] == "active"
    }

    pub async fn publish(
        &self,
        output: &mpsc::Sender<ProviderEvent>,
        tid: &str,
    ) -> Result<(), String> {
        output
            .send(ProviderEvent::Codex(
                json!({"method":"o3/goal","params":{"threadId":tid,"goal":self.current}}),
            ))
            .await
            .map_err(|_| "receiver closed".into())
    }

    async fn set(&mut self, client: &Client, tid: &str, params: Value) -> Result<(), String> {
        let mut params = params;
        params["threadId"] = json!(tid);
        if params.get("origin").is_none() {
            params["origin"] = json!(self.origin);
        }
        let response = client
            .call("thread/goal/set", params)
            .await
            .map_err(rejection)?;
        self.current = native_goal::codex(&response["goal"])?;
        Ok(())
    }

    /// Some(reply) completes a control-only app turn; None starts native work.
    pub async fn prepare(
        &mut self,
        client: &Client,
        tid: &str,
        command: &Command,
        output: &mpsc::Sender<ProviderEvent>,
    ) -> Result<Option<Option<String>>, String> {
        if let Command::Set(objective) = command {
            let length = objective.chars().count();
            if length > 4000 {
                return Err(format!("The goal objective is {length} characters; Codex supports at most 4000. Shorten it and send /goal again. The current goal was not changed."));
            }
        }
        let response = client
            .call("thread/goal/get", json!({"threadId":tid}))
            .await?;
        self.current = native_goal::codex(&response["goal"])?;
        let reply = match command {
            Command::Show => Some(if self.current.is_null() {
                "No goal is set.".into()
            } else {
                format!(
                    "Goal {}: {}",
                    self.current["status"]
                        .as_str()
                        .unwrap_or("")
                        .replace('_', " "),
                    self.current["objective"].as_str().unwrap_or("")
                )
            }),
            Command::Clear => {
                let result = client
                    .call(
                        "thread/goal/clear",
                        json!({"threadId":tid,"origin":self.origin}),
                    )
                    .await?;
                self.current = Value::Null;
                if result["cleared"] == true {
                    None
                } else {
                    Some("No goal is set.".into())
                }
            }
            Command::Pause | Command::Resume if self.current.is_null() => {
                Some("No goal is set.".into())
            }
            Command::Pause => {
                self.set(client, tid, json!({"status":"paused"})).await?;
                None
            }
            Command::Resume => {
                self.managed = true;
                return Ok(None);
            }
            Command::Set(objective) => {
                if !self.current.is_null() {
                    client
                        .call(
                            "thread/goal/clear",
                            json!({"threadId":tid,"origin":self.origin}),
                        )
                        .await?;
                    self.current = Value::Null;
                }
                self.managed = true;
                self.set(
                    client,
                    tid,
                    json!({"objective":objective,"status":"paused"}),
                )
                .await?;
                return Ok(None);
            }
        };
        self.publish(output, tid).await?;
        Ok(Some(reply))
    }

    pub async fn activated(
        &mut self,
        response: &Value,
        tid: &str,
        output: &mpsc::Sender<ProviderEvent>,
    ) -> Result<(), String> {
        // A terminal notification can precede the activation RPC's older snapshot.
        if !self.activation_updated {
            self.current = native_goal::codex(&response["goal"])?;
        }
        self.managed = false;
        self.publish(output, tid).await
    }

    pub async fn pause(
        &mut self,
        client: &Client,
        tid: &str,
        output: &mpsc::Sender<ProviderEvent>,
    ) -> Result<(), String> {
        if self.active() || self.managed {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.set(client, tid, json!({"status":"paused","origin":"automatic"})),
            )
            .await
            .map_err(|_| "Codex goal pause timed out")??;
            self.managed = false;
            self.publish(output, tid).await?;
        }
        Ok(())
    }

    pub async fn observe(
        &mut self,
        frame: &Value,
        output: &mpsc::Sender<ProviderEvent>,
        tid: &str,
    ) -> Result<bool, String> {
        if frame["method"] == "thread/goal/updated" && frame["params"]["goal"]["status"] == "active"
        {
            self.activation_observed = true;
        }
        if self.managed
            && !self.activation_observed
            && (frame["method"] == "thread/goal/cleared"
                || (frame["method"] == "thread/goal/updated"
                    && frame["params"]["goal"]["status"] == "paused"))
        {
            return Ok(true);
        }
        match frame["method"].as_str() {
            Some("thread/goal/updated") => {
                self.current = native_goal::codex(&frame["params"]["goal"])?
            }
            Some("thread/goal/cleared") => self.current = Value::Null,
            _ => return Ok(false),
        }
        self.activation_updated = true;
        if self.activation_observed && !self.active() {
            self.managed = false;
        }
        if matches!(
            self.current["status"].as_str(),
            Some("complete" | "blocked" | "usage_limited" | "budget_limited")
        ) {
            self.managed = false;
        }
        self.publish(output, tid).await?;
        Ok(true)
    }
}
