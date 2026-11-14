//! What sessions spent, kept across sessions and restarts for the usage
//! overview.
//!
//! acc dates usage replayed from a log to the session's start, since logs
//! have no times. So a session's record is taken whole only the first time
//! it is seen; after that, what its total grows by is added at the time it
//! was read, and the record keeps the days it was really spent on. The
//! runtime stores the records ([`Effect::SaveUsage`]) and hands them back
//! at the next start ([`App::load_usage`]).

use acc_core::event::Cost;
use acc_core::session::Session;
use acc_core::usage::{BUCKET_MS, Bucket, Spend, UsageRecord};
use serde::{Deserialize, Serialize};

use crate::event::Effect;
use crate::model::App;

/// Records are written at most this often while sessions spend.
const SAVE_EVERY_MS: u64 = 30_000;
/// Records with nothing newer than this are dropped when saving.
const KEEP_MS: u64 = 90 * 24 * 60 * 60 * 1000;

/// A session's running total in a unit other than dollars (Copilot's AI
/// Credits), as last reported. acc's records don't keep it, and it is
/// never added to dollars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OtherCost {
    pub cost: Cost,
    /// When it was read, ms since the epoch.
    pub at_ms: u64,
}

/// A record as saved: acc's, with the session's [`OtherCost`] beside it.
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(flatten)]
    record: UsageRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    other_cost: Option<OtherCost>,
}

fn total(r: &UsageRecord) -> Spend {
    let mut s = Spend::default();
    for b in &r.buckets {
        s.add(&b.spend);
    }
    s
}

/// What `now` has that `before` hadn't (never negative).
fn growth(now: &Spend, before: &Spend) -> Spend {
    Spend {
        input_tokens: now.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: now.output_tokens.saturating_sub(before.output_tokens),
        cache_read_tokens: now.cache_read_tokens.saturating_sub(before.cache_read_tokens),
        cache_write_tokens: now.cache_write_tokens.saturating_sub(before.cache_write_tokens),
        cost_usd: (now.cost_usd - before.cost_usd).max(0.0),
    }
}

/// The newest time a record says anything about.
fn newest(r: &UsageRecord) -> u64 {
    let spent = r.buckets.iter().map(|b| b.start_ms).max().unwrap_or(0);
    spent.max(r.quota_at_ms.unwrap_or(0))
}

/// Folds a session's latest record into the stored one.
pub(crate) fn fold(stored: &mut UsageRecord, latest: UsageRecord, now_ms: u64) {
    let grown = growth(&total(&latest), &total(stored));
    if !grown.is_zero() {
        let start = now_ms - now_ms % BUCKET_MS;
        match stored.buckets.iter_mut().find(|b| b.start_ms == start) {
            Some(b) => b.spend.add(&grown),
            None => {
                stored.buckets.push(Bucket {
                    start_ms: start,
                    spend: grown,
                });
                stored.buckets.sort_by_key(|b| b.start_ms);
            }
        }
    }
    if latest.quota.is_some() && latest.quota_at_ms >= stored.quota_at_ms {
        stored.quota = latest.quota;
        stored.quota_at_ms = latest.quota_at_ms;
    }
}

impl App {
    /// Records kept from earlier runs, as [`Effect::SaveUsage`] wrote them.
    pub fn load_usage(&mut self, json: &str) {
        let records: Vec<Saved> = serde_json::from_str(json).unwrap_or_default();
        for Saved { record, other_cost } in records {
            if let Some(c) = other_cost {
                self.other_costs.entry(record.session_id.clone()).or_insert(c);
            }
            self.usage.entry(record.session_id.clone()).or_insert(record);
        }
    }

    /// The local time zone's offset from UTC, that days are split in.
    pub fn set_utc_offset(&mut self, minutes: i32) {
        self.utc_offset_min = minutes;
    }

    /// Every record, the newest of each session.
    pub fn usage_records(&self) -> Vec<UsageRecord> {
        self.usage.values().cloned().collect()
    }

    /// What sessions picked by `pick` spent in units other than dollars,
    /// added up per unit.
    pub(crate) fn other_costs(&self, pick: impl Fn(&UsageRecord) -> bool) -> Vec<Cost> {
        let mut out: Vec<Cost> = Vec::new();
        for (id, c) in &self.other_costs {
            if !self.usage.get(id).is_some_and(&pick) {
                continue;
            }
            match out.iter_mut().find(|o| o.currency == c.cost.currency) {
                Some(o) => o.amount += c.cost.amount,
                None => out.push(c.cost.clone()),
            }
        }
        out
    }

    /// Takes in what a session reported spending.
    pub(crate) fn record_usage(&mut self, session: &Session) {
        if let Some(cost) = &session.usage().other_cost {
            self.other_costs.insert(
                session.id().to_owned(),
                OtherCost {
                    cost: cost.clone(),
                    at_ms: self.now_ms,
                },
            );
        }
        let latest = session.usage_record();
        match self.usage.get_mut(&latest.session_id) {
            Some(stored) => fold(stored, latest, self.now_ms),
            None => {
                self.usage.insert(latest.session_id.clone(), latest);
            }
        }
        self.usage_dirty = true;
    }

    /// Saves the records when they changed and it's time (or `now`).
    pub(crate) fn save_usage(&mut self, now: bool) -> Option<Effect> {
        if !self.usage_dirty || (!now && self.now_ms < self.usage_saved_ms + SAVE_EVERY_MS) {
            return None;
        }
        self.usage_dirty = false;
        self.usage_saved_ms = self.now_ms;
        let cutoff = self.now_ms.saturating_sub(KEEP_MS);
        let others = &self.other_costs;
        self.usage
            .retain(|id, r| newest(r).max(others.get(id).map_or(0, |c| c.at_ms)) >= cutoff);
        let usage = &self.usage;
        self.other_costs.retain(|id, _| usage.contains_key(id));
        let saved: Vec<Saved> = self
            .usage
            .values()
            .map(|r| Saved {
                record: r.clone(),
                other_cost: self.other_costs.get(&r.session_id).cloned(),
            })
            .collect();
        let json = serde_json::to_string(&saved).ok()?;
        Some(Effect::SaveUsage { json })
    }
}

#[cfg(test)]
mod tests {
    use acc_core::Harness;

    use super::*;

    fn rec(buckets: &[(u64, u64)]) -> UsageRecord {
        UsageRecord {
            session_id: "s1".into(),
            harness: Harness::Claude,
            buckets: buckets
                .iter()
                .map(|&(start_ms, output_tokens)| Bucket {
                    start_ms,
                    spend: Spend {
                        output_tokens,
                        ..Spend::default()
                    },
                })
                .collect(),
            quota: None,
            quota_at_ms: None,
        }
    }

    #[test]
    fn other_costs_are_saved_beside_the_records_and_added_up_per_unit() {
        let mut a = App::new(crate::config::Config::default(), "test");
        for (id, amount) in [("s1", 1.5), ("s2", 2.0)] {
            let mut r = rec(&[(0, 10)]);
            r.session_id = id.into();
            a.usage.insert(id.into(), r);
            a.other_costs.insert(
                id.into(),
                OtherCost {
                    cost: Cost {
                        amount,
                        currency: "AI Credits".into(),
                    },
                    at_ms: 0,
                },
            );
        }
        a.usage_dirty = true;
        let Some(Effect::SaveUsage { json }) = a.save_usage(true) else {
            panic!("saves");
        };
        let mut b = App::new(crate::config::Config::default(), "test");
        b.load_usage(&json);
        assert_eq!(b.other_costs, a.other_costs);
        assert_eq!(b.usage_records(), a.usage_records());
        assert_eq!(
            b.other_costs(|_| true),
            vec![Cost {
                amount: 3.5,
                currency: "AI Credits".into()
            }]
        );
        // Records saved before other costs were kept still load.
        let mut c = App::new(crate::config::Config::default(), "test");
        c.load_usage(&serde_json::to_string(&a.usage_records()).unwrap());
        assert_eq!(c.usage_records(), a.usage_records());
        assert!(c.other_costs.is_empty());
    }

    fn credits(amount: f64, currency: &str, at_ms: u64) -> OtherCost {
        OtherCost {
            cost: Cost {
                amount,
                currency: currency.into(),
            },
            at_ms,
        }
    }

    #[test]
    fn other_costs_are_added_per_unit_and_only_for_the_sessions_picked() {
        let mut a = App::new(crate::config::Config::default(), "test");
        for (id, harness, unit, amount) in [
            ("s1", Harness::Claude, "AI Credits", 1.0),
            ("s2", Harness::Claude, "AI Credits", 2.0),
            ("s3", Harness::Claude, "points", 4.0),
        ] {
            let mut r = rec(&[(0, 1)]);
            r.session_id = id.into();
            r.harness = harness;
            a.usage.insert(id.into(), r);
            a.other_costs.insert(id.into(), credits(amount, unit, 0));
        }
        // Units are never mixed.
        let mut all = a.other_costs(|_| true);
        all.sort_by(|x, y| x.currency.cmp(&y.currency));
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].currency.as_str(), all[0].amount), ("AI Credits", 3.0));
        assert_eq!((all[1].currency.as_str(), all[1].amount), ("points", 4.0));
        // Only the sessions the filter keeps count.
        assert!(a.other_costs(|r| r.harness == Harness::Codex).is_empty());
        // A cost with no usage record is ignored.
        a.other_costs
            .insert("ghost".into(), credits(9.0, "AI Credits", 0));
        assert_eq!(a.other_costs(|_| true).iter().map(|c| c.amount).sum::<f64>(), 7.0);
    }

    #[test]
    fn saving_drops_old_records_with_their_other_costs_but_keeps_recent_credits() {
        let now = 2 * KEEP_MS;
        let mut a = App::new(crate::config::Config::default(), "test");
        a.now_ms = now;
        // Old spend and old credits: dropped, along with its other cost.
        let mut old = rec(&[(0, 10)]);
        old.session_id = "old".into();
        a.usage.insert("old".into(), old);
        a.other_costs.insert("old".into(), credits(1.0, "AI Credits", 0));
        // Old spend, but credits read recently: kept.
        let mut recent = rec(&[(0, 10)]);
        recent.session_id = "recent".into();
        a.usage.insert("recent".into(), recent);
        a.other_costs
            .insert("recent".into(), credits(2.0, "AI Credits", now));
        a.usage_dirty = true;
        let Some(Effect::SaveUsage { json }) = a.save_usage(true) else {
            panic!("saves");
        };
        assert!(!a.usage.contains_key("old") && !a.other_costs.contains_key("old"));
        assert!(a.usage.contains_key("recent") && a.other_costs.contains_key("recent"));
        let mut b = App::new(crate::config::Config::default(), "test");
        b.load_usage(&json);
        assert_eq!(b.other_costs.keys().collect::<Vec<_>>(), vec!["recent"]);
        // Nothing changed since: nothing saved.
        assert!(a.save_usage(true).is_none());
    }

    #[test]
    fn loading_keeps_what_is_already_known() {
        let mut a = App::new(crate::config::Config::default(), "test");
        a.other_costs.insert("s1".into(), credits(5.0, "AI Credits", 1));
        let json = r#"[{"session_id":"s1","harness":"claude","buckets":[],"quota":null,"quota_at_ms":null,"other_cost":{"cost":{"amount":1.0,"currency":"AI Credits"},"at_ms":0}}]"#;
        a.load_usage(json);
        assert_eq!(a.other_costs["s1"].cost.amount, 5.0);
        // Garbage is ignored.
        a.load_usage("not json");
        assert_eq!(a.usage.len(), 1);
    }

    #[test]
    fn a_replayed_record_only_adds_its_growth_at_the_time_read() {
        let day = 24 * 60 * 60 * 1000;
        // Spent 100 yesterday, as recorded live.
        let mut stored = rec(&[(day, 100)]);
        // Reattached today: the replay dates all 100 to the start, plus 30 new.
        let now = 2 * day + 5 * BUCKET_MS + 7;
        fold(&mut stored, rec(&[(0, 130)]), now);
        assert_eq!(
            stored
                .buckets
                .iter()
                .map(|b| (b.start_ms, b.spend.output_tokens))
                .collect::<Vec<_>>(),
            vec![(day, 100), (2 * day + 5 * BUCKET_MS, 30)]
        );
        // Nothing new: nothing added.
        fold(&mut stored, rec(&[(0, 130)]), now + 1);
        assert_eq!(total(&stored).output_tokens, 130);
    }
}
