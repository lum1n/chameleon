//! What sessions spent, kept across sessions and restarts for the usage
//! overview.
//!
//! acc dates usage replayed from a log to the session's start, since logs
//! have no times. So a session's record is taken whole only the first time
//! it is seen; after that, what its total grows by is added at the time it
//! was read, and the record keeps the days it was really spent on. The
//! runtime stores the records ([`Effect::SaveUsage`]) and hands them back
//! at the next start ([`App::load_usage`]).

use acc_core::session::Session;
use acc_core::usage::{BUCKET_MS, Bucket, Spend, UsageRecord};

use crate::event::Effect;
use crate::model::App;

/// Records are written at most this often while sessions spend.
const SAVE_EVERY_MS: u64 = 30_000;
/// Records with nothing newer than this are dropped when saving.
const KEEP_MS: u64 = 90 * 24 * 60 * 60 * 1000;

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
        let records: Vec<UsageRecord> = serde_json::from_str(json).unwrap_or_default();
        for r in records {
            self.usage.entry(r.session_id.clone()).or_insert(r);
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

    /// Takes in what a session reported spending.
    pub(crate) fn record_usage(&mut self, session: &Session) {
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
        self.usage.retain(|_, r| newest(r) >= cutoff);
        let json = serde_json::to_string(&self.usage_records()).ok()?;
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
