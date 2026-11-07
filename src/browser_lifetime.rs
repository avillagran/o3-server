//! Native agent-tab idle, visibility, capacity and activity decisions.
//! Entries are in admission order; pending keys are unique reservations.
use serde::{Deserialize, Serialize};

pub(crate) const AGENT_LIMIT: usize = 8;
pub(crate) const SERVER_LIMIT: usize = 32;
pub(crate) const IDLE_MS: i64 = 30 * 60 * 1000;
pub(crate) const SWEEP_MS: u64 = 60_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tab {
    pub thread_id: String,
    pub tab_id: String,
    pub agent_id: Option<String>,
    pub desktop: bool,
    pub viewers: usize,
    pub shown: bool,
    pub used_at: i64,
    pub created_at: i64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pending {
    pub thread_id: String,
    pub tab_id: String,
    pub agent_id: Option<String>,
}

pub(crate) fn visible(tab: &Tab) -> bool {
    tab.viewers > 0 || (tab.desktop && tab.shown)
}
pub(crate) fn idle(tabs: &[Tab], now: i64) -> Vec<usize> {
    let cutoff = now.saturating_sub(IDLE_MS);
    tabs.iter()
        .enumerate()
        .filter_map(|(index, tab)| {
            (tab.agent_id.is_some() && !visible(tab) && tab.used_at < cutoff).then_some(index)
        })
        .collect()
}
pub(crate) fn headless_idle(tabs: &[Tab], now: i64) -> Vec<usize> {
    idle(tabs, now)
        .into_iter()
        .filter(|&index| !tabs[index].desktop)
        .collect()
}
pub(crate) fn capacity(tabs: &[Tab], pending: &[Pending], agent: &str) -> (bool, bool) {
    let reservations: Vec<_> = pending
        .iter()
        .filter(|pending| {
            !tabs
                .iter()
                .any(|tab| tab.thread_id == pending.thread_id && tab.tab_id == pending.tab_id)
        })
        .collect();
    (
        tabs.len() + reservations.len() >= SERVER_LIMIT,
        tabs.iter()
            .filter(|tab| tab.agent_id.as_deref() == Some(agent))
            .count()
            + reservations
                .iter()
                .filter(|pending| pending.agent_id.as_deref() == Some(agent))
                .count()
            >= AGENT_LIMIT,
    )
}
pub(crate) fn reclaim(tabs: &[Tab], pending: &[Pending], agent: &str, now: i64) -> Option<usize> {
    let (server_full, agent_full) = capacity(tabs, pending, agent);
    if !server_full || agent_full {
        return None;
    }
    // Strict comparison preserves admission order when timestamps tie, as JS's
    // stable Array.sort does for the native Map's values.
    idle(tabs, now)
        .into_iter()
        .fold(None, |oldest, index| match oldest {
            Some(previous) if tabs[previous].used_at <= tabs[index].used_at => Some(previous),
            _ => Some(index),
        })
}
pub(crate) fn latest(tabs: &[Tab], thread: &str, agent: Option<&str>) -> Option<usize> {
    let mut owned = None;
    let mut fallback = None;
    for (index, tab) in tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.thread_id == thread)
    {
        if agent.is_some()
            && tab.agent_id.as_deref() == agent
            && owned.is_none_or(|previous: usize| tabs[previous].created_at < tab.created_at)
        {
            owned = Some(index);
        }
        if fallback.is_none_or(|previous: usize| {
            (tab.viewers > 0, tab.used_at) > (tabs[previous].viewers > 0, tabs[previous].used_at)
        }) {
            fallback = Some(index);
        }
    }
    owned.or(fallback)
}
pub(crate) fn used_indices(
    tabs: &[Tab],
    thread: &str,
    agent: &str,
    explicit: Option<&str>,
) -> Vec<usize> {
    tabs.iter()
        .enumerate()
        .filter_map(|(index, tab)| {
            (tab.thread_id == thread
                && (tab.agent_id.as_deref() == Some(agent)
                    || explicit == Some(tab.tab_id.as_str())))
            .then_some(index)
        })
        .collect()
}
