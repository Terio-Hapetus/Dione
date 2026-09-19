//! M9a metrics: agent-agnostic usage accounting (per task/agent/model).
//!
//! Legacy `Cost` (server-absolute dollars) stays frozen; this module adds
//! [`UsageSample`] where `cost: None` means "tokens known, dollars unknown"
//! (terminal CLIs, CodexBar-style local-log parsing). Aggregation is pure:
//! the log owns samples, views fold over quota windows (5h/day/week).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::transcript::{Cost, TaskId};

/// Quota-style window sizes in seconds (CodexBar 5h/weekly cadence).
pub const WINDOW_5H_SECS: u64 = 5 * 3600;
pub const WINDOW_DAY_SECS: u64 = 24 * 3600;
pub const WINDOW_WEEK_SECS: u64 = 7 * 24 * 3600;

/// Max samples kept in-memory (U2): `collect_usage` runs on the 160ms UI
/// loop, so an unbounded log would grow the per-tick clone forever.
/// Oldest samples evict first (FIFO); windowed views (5h/day/week) are
/// unaffected in practice.
pub const METRICS_CAP: usize = 2048;

/// One usage observation. `agent`/`model` are attribution dimensions
/// (`Task::{agent_ref, model_override}` at record time); empty model means
/// "unknown / session default".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSample {
    pub task: TaskId,
    pub agent: String,
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache: f64,
    /// Server-reported dollars, if any. `None` = terminal path: real
    /// tokens, unknown money (UI must render `~`/n/a, never $0).
    pub cost: Option<f64>,
    /// Unix seconds. Clock skew tolerated: windows compare saturating.
    pub ts: u64,
}

impl UsageSample {
    pub fn new(task: TaskId, agent: &str, model: &str) -> Self {
        Self {
            task,
            agent: agent.to_string(),
            model: model.to_string(),
            input: 0.0,
            output: 0.0,
            cache: 0.0,
            cost: None,
            ts: now_unix(),
        }
    }

    /// Bridge from a legacy server-absolute [`Cost`] (opencode path).
    pub fn from_cost(task: TaskId, agent: &str, model: &str, cost: Cost, ts: u64) -> Self {
        Self {
            task,
            agent: agent.to_string(),
            model: model.to_string(),
            input: cost.input,
            output: cost.output,
            cache: cost.cache,
            cost: Some(cost.cost),
            ts,
        }
    }

    pub fn tokens(&self) -> f64 {
        self.input + self.output + self.cache
    }
}

/// Folded totals over a set of samples.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageTotals {
    pub input: f64,
    pub output: f64,
    pub cache: f64,
    /// Sum of known costs only; see `cost_unknown`.
    pub cost: f64,
    /// Any folded sample lacked dollars → `cost` is a lower bound.
    pub cost_unknown: bool,
    pub samples: u64,
}

impl UsageTotals {
    pub fn add(&mut self, s: &UsageSample) {
        self.input += s.input;
        self.output += s.output;
        self.cache += s.cache;
        match s.cost {
            Some(c) => self.cost += c,
            None => self.cost_unknown = true,
        }
        self.samples += 1;
    }

    pub fn tokens(&self) -> f64 {
        self.input + self.output + self.cache
    }
}

/// Append-only usage log. Bounded by caller session lifetime (in-memory,
/// like `Store.blocked`); persistence is a later slice if needed.
#[derive(Debug, Clone, Default)]
pub struct MetricsLog {
    samples: Vec<UsageSample>,
}

impl MetricsLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, s: UsageSample) {
        self.samples.push(s);
        if self.samples.len() > METRICS_CAP {
            self.samples.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Borrowed snapshot for read-out seams (fleet → UI). Clone at the
    /// boundary; the log itself stays append-only.
    pub fn samples(&self) -> &[UsageSample] {
        &self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Fold samples inside the window (`None` = all time) matching `filter`.
    /// A sample is inside when `now - ts <= window` (saturating; future
    /// timestamps count as now).
    pub fn aggregate(
        &self,
        now: u64,
        window_secs: Option<u64>,
        filter: impl Fn(&UsageSample) -> bool,
    ) -> UsageTotals {
        let mut out = UsageTotals::default();
        for s in &self.samples {
            if !filter(s) {
                continue;
            }
            if let Some(w) = window_secs
                && now.saturating_sub(s.ts) > w
            {
                continue;
            }
            out.add(s);
        }
        out
    }

    /// Fold per key over a window (`None` = all time).
    pub fn group_by<K: Ord>(
        &self,
        now: u64,
        window_secs: Option<u64>,
        key: impl Fn(&UsageSample) -> K,
    ) -> BTreeMap<K, UsageTotals> {
        let mut map: BTreeMap<K, UsageTotals> = BTreeMap::new();
        for s in &self.samples {
            if let Some(w) = window_secs
                && now.saturating_sub(s.ts) > w
            {
                continue;
            }
            map.entry(key(s)).or_default().add(s);
        }
        map
    }

    pub fn by_task(&self, now: u64, window_secs: Option<u64>) -> BTreeMap<TaskId, UsageTotals> {
        self.group_by(now, window_secs, |s| s.task)
    }

    pub fn by_agent(&self, now: u64, window_secs: Option<u64>) -> BTreeMap<String, UsageTotals> {
        self.group_by(now, window_secs, |s| s.agent.clone())
    }

    pub fn by_model(&self, now: u64, window_secs: Option<u64>) -> BTreeMap<String, UsageTotals> {
        self.group_by(now, window_secs, |s| s.model.clone())
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(task: TaskId, agent: &str, model: &str, ts: u64, cost: Option<f64>) -> UsageSample {
        UsageSample {
            task,
            agent: agent.into(),
            model: model.into(),
            input: 10.0,
            output: 5.0,
            cache: 2.0,
            cost,
            ts,
        }
    }

    #[test]
    fn record_and_all_time_totals() {
        let mut log = MetricsLog::new();
        assert!(log.is_empty());
        let t = TaskId::new();
        log.record(sample(t, "opencode", "gpt", 1000, Some(0.5)));
        log.record(sample(t, "opencode", "gpt", 1000, None));
        assert_eq!(log.len(), 2);
        let tot = log.aggregate(2000, None, |_| true);
        assert_eq!(tot.input, 20.0);
        assert_eq!(tot.tokens(), 34.0);
        // Unknown dollars poison the sum into a lower bound, never $0.
        assert_eq!(tot.cost, 0.5);
        assert!(tot.cost_unknown);
        assert_eq!(tot.samples, 2);
    }

    #[test]
    fn windows_drop_old_samples() {
        let mut log = MetricsLog::new();
        let t = TaskId::new();
        log.record(sample(t, "claude", "m", 100, Some(1.0)));
        log.record(sample(t, "claude", "m", 900, Some(2.0)));
        let now = 1000;
        assert_eq!(log.aggregate(now, Some(200), |_| true).cost, 2.0);
        assert_eq!(log.aggregate(now, Some(WINDOW_5H_SECS), |_| true).cost, 3.0);
        assert_eq!(log.aggregate(now, None, |_| true).samples, 2);
        // Future timestamps count as now (clock skew), never dropped.
        log.record(sample(t, "claude", "m", now + 500, Some(4.0)));
        assert_eq!(log.aggregate(now, Some(0), |_| true).cost, 4.0);
    }

    #[test]
    fn groups_split_by_task_agent_model() {
        let mut log = MetricsLog::new();
        let a = TaskId::new();
        let b = TaskId::new();
        log.record(sample(a, "opencode", "gpt", 10, Some(1.0)));
        log.record(sample(b, "claude", "sonnet", 10, None));
        let by_task = log.by_task(20, None);
        assert_eq!(by_task.len(), 2);
        assert!(!by_task[&a].cost_unknown);
        assert!(by_task[&b].cost_unknown);
        let by_agent = log.by_agent(20, None);
        assert_eq!(by_agent["opencode"].samples, 1);
        assert_eq!(by_agent["claude"].tokens(), 17.0);
        let by_model = log.by_model(20, Some(WINDOW_WEEK_SECS));
        assert_eq!(by_model["gpt"].cost, 1.0);
        assert_eq!(by_model["sonnet"].cost, 0.0);
    }

    #[test]
    fn from_cost_bridge_keeps_dollars() {
        let t = TaskId::new();
        let s = UsageSample::from_cost(
            t,
            "opencode",
            "gpt",
            Cost {
                input: 5.0,
                output: 1.0,
                cache: 0.0,
                cost: 0.1,
            },
            42,
        );
        assert_eq!(s.cost, Some(0.1));
        assert_eq!(s.tokens(), 6.0);
        let mut log = MetricsLog::new();
        log.record(s);
        let tot = log.aggregate(42, None, |_| true);
        assert_eq!(tot.cost, 0.1);
        assert!(!tot.cost_unknown);
    }

    #[test]
    fn empty_log_folds_to_default() {
        let log = MetricsLog::new();
        assert_eq!(log.aggregate(0, None, |_| true), UsageTotals::default());
        assert!(log.by_agent(0, None).is_empty());
    }

    #[test]
    fn record_caps_fifo() {
        let mut log = MetricsLog::new();
        let t = TaskId::new();
        for n in 0..METRICS_CAP + 10 {
            log.record(sample(t, "a", "m", n as u64, None));
        }
        assert_eq!(log.len(), METRICS_CAP);
        // Oldest 10 evicted: first kept sample has ts == 10.
        assert_eq!(log.samples()[0].ts, 10);
        assert_eq!(log.samples()[METRICS_CAP - 1].ts, (METRICS_CAP + 9) as u64);
    }
}
