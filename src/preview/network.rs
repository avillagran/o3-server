//! Native browser request, diagnostic and aborted-navigation state transitions.
use super::navigation::{slice_utf16, without_data_payload};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
#[derive(Clone)]
struct Request {
    url: String,
    method: String,
    main: bool,
    generation: Option<u64>,
}
#[derive(Clone)]
pub(crate) struct Aborted {
    pub url: String,
    pub generation: u64,
    pub token: u64,
}
#[derive(Default)]
pub(crate) struct Transition {
    pub reports: Vec<Value>,
    pub loaded_calls: u64,
}
pub(crate) struct Kernel {
    pub generation: u64,
    pub loading: bool,
    pub closing: bool,
    pub aborted: Option<Aborted>,
    page_url: String,
    requests: BTreeMap<String, Request>,
    network: VecDeque<Value>,
    next_timer: u64,
}
fn text<'a>(value: &'a Value, name: &str, fallback: &'a str) -> &'a str {
    value[name].as_str().unwrap_or(fallback)
}
pub(crate) fn parse_error(error: &str) -> (i64, String) {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let description = PATTERN
        .get_or_init(|| regex::Regex::new("ERR_[A-Z0-9_]+").unwrap())
        .find(error)
        .map_or(error, |value| value.as_str());
    let code = match description {
        "ERR_FAILED" => -2,
        "ERR_TIMED_OUT" => -7,
        "ERR_CONNECTION_CLOSED" => -100,
        "ERR_CONNECTION_RESET" => -101,
        "ERR_CONNECTION_REFUSED" => -102,
        "ERR_NAME_NOT_RESOLVED" => -105,
        "ERR_INTERNET_DISCONNECTED" => -106,
        "ERR_ADDRESS_UNREACHABLE" => -109,
        "ERR_CERT_AUTHORITY_INVALID" => -202,
        "ERR_EMPTY_RESPONSE" => -324,
        _ => -2,
    };
    (code, description.into())
}
impl Kernel {
    pub fn new(page_url: String) -> Self {
        Self {
            generation: 0,
            loading: false,
            closing: false,
            aborted: None,
            page_url,
            requests: BTreeMap::new(),
            network: VecDeque::new(),
            next_timer: 0,
        }
    }
    pub fn entries(&self) -> Vec<Value> {
        self.network.iter().cloned().collect()
    }
    fn record(&mut self, step: &Value) -> Request {
        self.requests
            .entry(text(step, "id", "").into())
            .or_insert_with(|| Request {
                url: text(step, "url", "https://example.com/").into(),
                method: text(step, "verb", "GET").into(),
                main: step["main"] != false && step["navigation"] != false,
                generation: None,
            })
            .clone()
    }
    fn push(&mut self, value: Value) {
        self.network.push_back(value);
        while self.network.len() > 200 {
            self.network.pop_front();
        }
    }
    fn settle(
        &mut self,
        generation: u64,
        url: &str,
        offered: Option<&Value>,
        transition: &mut Transition,
    ) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        if self.page_url != "about:blank" {
            transition.loaded_calls += 1;
            return;
        }
        let mut nav = json!({"_tag":"LoadFailed","url":slice_utf16(&without_data_payload(url),2048),"title":"","code":-3,"description":"ERR_ABORTED"});
        if let Some(offered) = offered {
            nav["download"] = offered.clone();
        }
        transition.reports.push(nav);
    }
    pub fn apply(&mut self, step: &Value, at: &str) -> Transition {
        let mut transition = Transition::default();
        match text(step, "kind", "") {
            "request" => {
                let request = self.record(step);
                if request.main {
                    self.generation += 1;
                    self.requests
                        .get_mut(text(step, "id", ""))
                        .unwrap()
                        .generation = Some(self.generation);
                    self.aborted = None;
                    self.loading = true;
                    transition.reports.push(json!({"_tag":"Loading","url":slice_utf16(&without_data_payload(&request.url),2048),"title":""}));
                }
            }
            "response" => {
                let request = self.record(step);
                self.push(json!({"url":without_data_payload(text(step,"url",&request.url)),"method":request.method,"status":step["status"],"failed":false,"timestamp":at}));
            }
            "failed" => {
                let request = self.record(step);
                let error = text(step, "errorText", "");
                self.push(json!({"url":without_data_payload(&request.url),"method":request.method,"status":null,"failed":true,"errorText":error,"timestamp":at}));
                if request.main && request.generation.unwrap_or(0) == self.generation {
                    if error.contains("ERR_ABORTED") {
                        self.next_timer += 1;
                        self.aborted = Some(Aborted {
                            url: request.url,
                            generation: self.generation,
                            token: self.next_timer,
                        });
                    } else {
                        self.loading = false;
                        let (code, description) = parse_error(error);
                        transition.reports.push(json!({"_tag":"LoadFailed","url":slice_utf16(&without_data_payload(&request.url),2048),"title":"","code":code,"description":description}));
                    }
                }
            }
            "load" => {
                self.loading = false;
                transition.loaded_calls += 1;
            }
            "frame" => {
                if step["main"] != false && !self.loading {
                    transition.loaded_calls += 1;
                }
            }
            "closing" => self.closing = step["value"] == true,
            "pageUrl" => self.page_url = text(step, "url", "").into(),
            "tick" => {
                if let Some(aborted) = self.aborted.take() {
                    if !self.closing {
                        self.settle(aborted.generation, &aborted.url, None, &mut transition);
                    }
                }
            }
            "download" => {
                if let Some(aborted) = self
                    .aborted
                    .clone()
                    .filter(|aborted| aborted.url == text(step, "url", ""))
                {
                    self.aborted = None;
                    self.settle(
                        aborted.generation,
                        &aborted.url,
                        step.get("offered"),
                        &mut transition,
                    );
                }
            }
            _ => {}
        }
        transition
    }
    pub fn replace_request(&mut self, id: &str) {
        self.requests.remove(id);
    }
    pub fn forget(&mut self, id: &str) {
        self.requests.remove(id);
    }
    pub fn has_request(&self, id: &str) -> bool {
        self.requests.contains_key(id)
    }
    #[cfg(test)]
    pub fn snapshot(&self, transition: &Transition) -> Value {
        json!({"networkLength":self.network.len(),"lastNetwork":self.network.back(),"generation":self.generation,"loading":self.loading,"reports":transition.reports,"loadedCalls":transition.loaded_calls,"aborted":self.aborted.as_ref().map(|value|json!({"url":value.url,"generation":value.generation})),"timerDelays":if self.aborted.is_some(){vec![5000]}else{vec![]}})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_native_network_failure_and_abort_callbacks_match() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/preview-network-4288.json"
        ))
        .unwrap();
        assert_eq!(fixture["cases"].as_array().unwrap().len(), 263);
        for (case_index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let mut kernel = Kernel::new(text(&case["input"], "pageUrl", "about:blank").into());
            for (step_index, step) in case["input"]["steps"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let transition = kernel.apply(
                    step,
                    &crate::store::iso(1794009600000 + step_index as i64 * 1000),
                );
                assert_eq!(
                    kernel.snapshot(&transition),
                    case["expected"]["steps"][step_index],
                    "native case {case_index}, step {step_index}"
                );
            }
            assert_eq!(
                json!(kernel.entries()),
                case["expected"]["networkEntries"],
                "native history {case_index}"
            );
        }
    }
}
