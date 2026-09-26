//! Health status: what the append-only health log proves, and when it goes
//! stale. Pure functions of (records, now) so tests never need time travel
//! and `warden health` stays a cheap read-only command.

use crate::object::{HealthEvent, HealthRecord};

#[derive(Debug, Default)]
pub struct HealthStatus {
    pub records: u64,
    pub last_fast_verify: Option<u64>,
    pub last_deep_verify: Option<u64>,
    pub last_readback: Option<u64>,
    pub last_drill: Option<u64>,
    pub last_prune: Option<u64>,
    /// From the most recent record (any event type).
    pub latest_problems_total: u64,
    pub latest_problems_sample: Vec<String>,
}

/// Staleness thresholds in days; 0 disables that clock.
pub struct StalePolicy {
    pub verify_days: u64,
    pub drill_days: u64,
}

impl Default for StalePolicy {
    fn default() -> Self {
        StalePolicy {
            verify_days: 30,
            drill_days: 90,
        }
    }
}

pub fn status_of(records: &[HealthRecord]) -> HealthStatus {
    let mut st = HealthStatus {
        records: records.len() as u64,
        ..Default::default()
    };
    let mut latest_time = 0u64;
    for r in records {
        let slot = match &r.event {
            HealthEvent::Verify { deep: true, .. } => &mut st.last_deep_verify,
            HealthEvent::Verify { deep: false, .. } => &mut st.last_fast_verify,
            HealthEvent::ReadBack { .. } => &mut st.last_readback,
            HealthEvent::Drill { .. } => &mut st.last_drill,
            HealthEvent::Prune { .. } => &mut st.last_prune,
        };
        *slot = Some(slot.map_or(r.time_unix, |t: u64| t.max(r.time_unix)));
        // Pessimistic tie-break: when records share the latest instant
        // (verify-then-drill within one second is routine), the WORST
        // finding wins. A clean drill must never mask a red verify.
        let newer = r.time_unix > latest_time;
        let worse_tie = r.time_unix == latest_time && r.problems_total > st.latest_problems_total;
        if newer || worse_tie {
            latest_time = r.time_unix;
            st.latest_problems_total = r.problems_total;
            st.latest_problems_sample = r.problems_sample.clone();
        }
    }
    st
}

/// Human-readable reasons this repo's health is NOT currently trustworthy.
/// Empty vec == healthy. The verify clock accepts either tier (deep is the
/// stronger proof and what the docs recommend scheduling).
pub fn staleness(st: &HealthStatus, policy: &StalePolicy, now_unix: u64) -> Vec<String> {
    let mut reasons = Vec::new();
    if st.latest_problems_total > 0 {
        reasons.push(format!(
            "latest run found {} problems",
            st.latest_problems_total
        ));
    }
    let day = 86_400u64;
    if policy.verify_days > 0 {
        match st.last_deep_verify.max(st.last_fast_verify) {
            None => reasons.push("never verified".into()),
            Some(t) if now_unix.saturating_sub(t) > policy.verify_days * day => {
                reasons.push(format!(
                    "verify overdue: {} days ago",
                    now_unix.saturating_sub(t) / day
                ))
            }
            Some(_) => {}
        }
    }
    if policy.drill_days > 0 {
        match st.last_drill {
            None => reasons.push("never drilled".into()),
            Some(t) if now_unix.saturating_sub(t) > policy.drill_days * day => {
                reasons.push(format!(
                    "drill overdue: {} days ago",
                    now_unix.saturating_sub(t) / day
                ))
            }
            Some(_) => {}
        }
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(t: u64, event: HealthEvent, problems: u64) -> HealthRecord {
        HealthRecord {
            version: 0,
            time_unix: t,
            event,
            problems_total: problems,
            problems_sample: if problems > 0 {
                vec!["p".into()]
            } else {
                vec![]
            },
        }
    }
    fn verify(t: u64, deep: bool, problems: u64) -> HealthRecord {
        rec(
            t,
            HealthEvent::Verify {
                deep,
                snapshots: 1,
                trees: 1,
                chunks: 1,
                orphans: 0,
            },
            problems,
        )
    }
    fn drill(t: u64) -> HealthRecord {
        rec(
            t,
            HealthEvent::Drill {
                snapshot: "ab".into(),
                files: 1,
                bytes: 10,
            },
            0,
        )
    }

    const DAY: u64 = 86_400;

    #[test]
    fn status_tracks_latest_per_event_and_latest_problems() {
        let st = status_of(&[verify(100, true, 0), verify(500, false, 2), drill(300)]);
        assert_eq!(st.records, 3);
        assert_eq!(st.last_deep_verify, Some(100));
        assert_eq!(st.last_fast_verify, Some(500));
        assert_eq!(st.last_drill, Some(300));
        assert_eq!(st.last_readback, None);
        assert_eq!(
            st.latest_problems_total, 2,
            "latest record (t=500) had problems"
        );
    }

    #[test]
    fn fresh_and_clean_is_healthy() {
        let now = 1000 * DAY;
        let st = status_of(&[verify(now - DAY, true, 0), drill(now - 2 * DAY)]);
        assert!(staleness(&st, &StalePolicy::default(), now).is_empty());
    }

    #[test]
    fn overdue_never_and_problems_all_produce_reasons() {
        let now = 1000 * DAY;
        // Overdue on both clocks.
        let st = status_of(&[verify(now - 40 * DAY, true, 0), drill(now - 100 * DAY)]);
        let r = staleness(&st, &StalePolicy::default(), now);
        assert!(r.iter().any(|x| x.contains("verify overdue")), "{r:?}");
        assert!(r.iter().any(|x| x.contains("drill overdue")), "{r:?}");
        // Nothing ever recorded.
        let r = staleness(&status_of(&[]), &StalePolicy::default(), now);
        assert!(r.iter().any(|x| x.contains("never verified")), "{r:?}");
        assert!(r.iter().any(|x| x.contains("never drilled")), "{r:?}");
        // Latest run found problems.
        let st = status_of(&[verify(now - DAY, true, 3), drill(now - DAY)]);
        let r = staleness(&st, &StalePolicy::default(), now);
        assert!(r.iter().any(|x| x.contains("found 3 problems")), "{r:?}");
        // Disabled clocks stay silent.
        let r = staleness(
            &status_of(&[]),
            &StalePolicy {
                verify_days: 0,
                drill_days: 0,
            },
            now,
        );
        assert!(r.is_empty(), "{r:?}");
    }
}
