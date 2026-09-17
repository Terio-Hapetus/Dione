//! M9d control room: usage per agent/model from drained `UsageSample`s.
//!
//! Data path: `Supervisor.metrics` → `FleetInbox::collect_usage` →
//! `DioneApp::usage` (160ms loop) → [`summarize`] (pure, tested) → rows.
//! Dollars render `~`-prefixed when any sample lacked them, `n/a` when
//! no sample had any — never a fake `$0`.

use std::collections::BTreeMap;

use base::{UsageSample, UsageTotals, WINDOW_5H_SECS, now_unix};
use gpui::*;
use gpui_component::{ActiveTheme as _, label::Label};

use super::theme::{empty_state, fmt_money, fmt_tok, muted_for, truncate};
use crate::app::DioneApp;

/// One attribution row: all-time totals + trailing-5h tokens.
pub(crate) struct CostGroup {
    pub key: String,
    pub all: UsageTotals,
    pub window_tokens: f64,
}

pub(crate) struct CostSummary {
    pub total: UsageTotals,
    pub total_5h: f64,
    pub by_agent: Vec<CostGroup>,
    pub by_model: Vec<CostGroup>,
}

/// Fold samples into a sorted summary (tokens desc). Pure: the view is a
/// direct render of this, and tests pin the money rules here.
pub(crate) fn summarize(samples: &[UsageSample], now: u64) -> CostSummary {
    fn fold(
        map: &mut BTreeMap<String, (UsageTotals, f64)>,
        key: String,
        s: &UsageSample,
        now: u64,
    ) {
        let e = map.entry(key).or_insert((UsageTotals::default(), 0.0));
        e.0.add(s);
        if now.saturating_sub(s.ts) <= WINDOW_5H_SECS {
            e.1 += s.tokens();
        }
    }
    let mut total = UsageTotals::default();
    let mut total_5h = 0.0;
    let mut agents: BTreeMap<String, (UsageTotals, f64)> = BTreeMap::new();
    let mut models: BTreeMap<String, (UsageTotals, f64)> = BTreeMap::new();
    for s in samples {
        total.add(s);
        let agent = if s.agent.is_empty() { "?" } else { &s.agent };
        let model = if s.model.is_empty() { "?" } else { &s.model };
        fold(&mut agents, agent.to_string(), s, now);
        fold(&mut models, model.to_string(), s, now);
        if now.saturating_sub(s.ts) <= WINDOW_5H_SECS {
            total_5h += s.tokens();
        }
    }
    fn sorted(map: BTreeMap<String, (UsageTotals, f64)>) -> Vec<CostGroup> {
        let mut v: Vec<CostGroup> = map
            .into_iter()
            .map(|(key, (all, window_tokens))| CostGroup {
                key,
                all,
                window_tokens,
            })
            .collect();
        v.sort_by(|a, b| {
            b.all
                .tokens()
                .partial_cmp(&a.all.tokens())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }
    CostSummary {
        total,
        total_5h,
        by_agent: sorted(agents),
        by_model: sorted(models),
    }
}

/// Money cell: exact when every sample had dollars, `~` lower-bound when
/// some lacked them, `n/a` for tokens-only totals.
pub(crate) fn fmt_cost(t: &UsageTotals) -> String {
    if t.samples == 0 {
        "—".to_string()
    } else if !t.cost_unknown {
        fmt_money(t.cost)
    } else if t.cost > 0.0 {
        format!("~{}", fmt_money(t.cost))
    } else {
        "n/a".to_string()
    }
}

impl DioneApp {
    pub(crate) fn render_costs(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        let border = cx.theme().border;
        if self.usage.is_empty() {
            return empty_state(
                "◔",
                "No usage yet",
                "Costs appear as agents run tasks",
                dark,
            );
        }
        let sum = summarize(&self.usage, now_unix());
        let mut col = div().flex().flex_col().gap_1().child(
            Label::new(format!(
                "Σ {} tok · {} (5h: {})",
                fmt_tok(sum.total.tokens()),
                fmt_cost(&sum.total),
                fmt_tok(sum.total_5h),
            ))
            .text_color(muted_for(dark)),
        );
        for (title, groups) in [("by agent", &sum.by_agent), ("by model", &sum.by_model)] {
            let mut section = div().flex().flex_col().gap_0p5().child(
                Label::new(title.to_string())
                    .text_size(px(10.))
                    .text_color(muted_for(dark)),
            );
            for g in groups {
                section = section.child(
                    div()
                        .rounded_sm()
                        .border_1()
                        .border_color(border)
                        .px_2()
                        .py_1()
                        .flex()
                        .justify_between()
                        .child(Label::new(truncate(&g.key, 24)))
                        .child(
                            Label::new(format!(
                                "{} tok · {} · 5h {}",
                                fmt_tok(g.all.tokens()),
                                fmt_cost(&g.all),
                                fmt_tok(g.window_tokens),
                            ))
                            .text_size(px(11.))
                            .text_color(muted_for(dark)),
                        ),
                );
            }
            col = col.child(section);
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // NOTE: no `use super::*` — this file's `use gpui::*` glob would
    // shadow builtin `#[test]` (same pitfall as vm_badge/top_bar).
    use base::{TaskId, UsageSample};

    use crate::views::costs::{fmt_cost, summarize};

    fn sample(agent: &str, model: &str, ts: u64, cost: Option<f64>) -> UsageSample {
        UsageSample {
            task: TaskId::new(),
            agent: agent.into(),
            model: model.into(),
            input: 10.0,
            output: 5.0,
            cache: 0.0,
            cost,
            ts,
        }
    }

    #[test]
    fn summary_groups_sorts_and_windows() {
        let now = 200_000;
        let samples = vec![
            sample("claude", "sonnet", now - 100, None),
            sample("opencode", "gpt", now - 100, Some(0.5)),
            sample("opencode", "gpt", now - 100_000, Some(1.0)),
        ];
        let sum = summarize(&samples, now);
        assert_eq!(sum.total.samples, 3);
        assert!(sum.total.cost_unknown);
        // Trailing 5h drops the old sample.
        assert_eq!(sum.total_5h, 30.0);
        // Sorted tokens desc: opencode (30) before claude (15).
        assert_eq!(sum.by_agent[0].key, "opencode");
        assert_eq!(sum.by_agent[0].window_tokens, 15.0);
        assert_eq!(sum.by_model.len(), 2);
    }

    #[test]
    fn money_cells_follow_the_rules() {
        let priced = crate::views::costs::summarize(&[sample("a", "m", 1, Some(0.02))], 2);
        assert_eq!(fmt_cost(&priced.total), "$0.02");
        let mixed = summarize(
            &[sample("a", "m", 1, Some(0.02)), sample("b", "n", 1, None)],
            2,
        );
        assert_eq!(fmt_cost(&mixed.total), "~$0.02");
        let tokens_only = summarize(&[sample("b", "n", 1, None)], 2);
        assert_eq!(fmt_cost(&tokens_only.total), "n/a");
        assert_eq!(fmt_cost(&summarize(&[], 2).total), "—");
    }
}
