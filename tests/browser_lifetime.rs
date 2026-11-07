#[path = "../src/browser_lifetime.rs"]
mod lifetime;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    tabs: Vec<lifetime::Tab>,
    pending: Vec<lifetime::Pending>,
    now: i64,
    thread: String,
    agent: String,
    latest_agent: Option<String>,
    explicit: Option<String>,
}

#[test]
fn complete_native_lifetime_helpers_match_capacity_visibility_ties_and_activity() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/browser-lifetime-4301.json")).unwrap();
    assert_eq!(
        lifetime::AGENT_LIMIT,
        fixture["constants"]["agent"].as_u64().unwrap() as usize
    );
    assert_eq!(
        lifetime::SERVER_LIMIT,
        fixture["constants"]["server"].as_u64().unwrap() as usize
    );
    assert_eq!(
        lifetime::IDLE_MS,
        fixture["constants"]["idle"].as_i64().unwrap()
    );
    assert_eq!(fixture["constants"]["sweep"], "1 minute");
    assert_eq!(lifetime::SWEEP_MS, 60_000);
    for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let mut input: Input = serde_json::from_value(row["input"].clone()).unwrap();
        let (server_full, agent_full) =
            lifetime::capacity(&input.tabs, &input.pending, &input.agent);
        let idle = lifetime::idle(&input.tabs, input.now);
        let headless = lifetime::headless_idle(&input.tabs, input.now);
        let reclaim: Vec<_> =
            lifetime::reclaim(&input.tabs, &input.pending, &input.agent, input.now)
                .into_iter()
                .collect();
        let latest = lifetime::latest(&input.tabs, &input.thread, input.latest_agent.as_deref());
        for selected in lifetime::used_indices(
            &input.tabs,
            &input.thread,
            &input.agent,
            input.explicit.as_deref(),
        ) {
            input.tabs[selected].used_at = input.now;
        }
        let marked: Vec<_> = input.tabs.iter().map(|tab| tab.used_at).collect();
        let actual = serde_json::json!({"serverFull":server_full,"agentFull":agent_full,"idle":idle,"headless":headless,"reclaim":reclaim,"latest":latest,"marked":marked});
        assert_eq!(actual, row["result"], "native observation {index}");
    }
}
