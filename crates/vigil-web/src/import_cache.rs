//! Import plans that were previewed and are waiting for the button.
//!
//! A plan holds what the file said, including notification addresses, so it stays in memory,
//! is small in number and short-lived, and is handed out once: pressing the button takes it.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use vigil_import::ImportPlan;

/// Plans that are kept.
const MAX_ENTRIES: usize = 4;

/// How long a plan waits.
const TTL: Duration = Duration::from_secs(15 * 60);

struct Entry {
    id: String,
    made: Instant,
    plan: ImportPlan,
}

/// The plans waiting for confirmation.
#[derive(Default)]
pub(crate) struct ImportPlans {
    entries: Mutex<Vec<Entry>>,
}

impl std::fmt::Debug for ImportPlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImportPlans")
    }
}

impl ImportPlans {
    /// Keep a plan and return the id that asks for it back. The oldest is forgotten when there
    /// are too many.
    pub(crate) fn put(&self, id: String, plan: ImportPlan, now: Instant) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| now.saturating_duration_since(entry.made) < TTL);
        if entries.len() >= MAX_ENTRIES {
            entries.remove(0);
        }
        entries.push(Entry {
            id,
            made: now,
            plan,
        });
    }

    /// The plan with this id, if it is still good. It is not given out twice.
    pub(crate) fn take(&self, id: &str, now: Instant) -> Option<ImportPlan> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let at = entries.iter().position(|entry| entry.id == id)?;
        let entry = entries.remove(at);
        (now.saturating_duration_since(entry.made) < TTL).then_some(entry.plan)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use vigil_import::{Existing, Options, plan};

    use super::ImportPlans;

    fn a_plan() -> vigil_import::ImportPlan {
        plan(
            b"https://a.example/\n",
            None,
            &Options::default(),
            &Existing::default(),
        )
        .unwrap_or_else(|_| unreachable!("a list of one address is a plan"))
    }

    #[test]
    fn a_plan_is_given_out_once_and_only_while_it_is_fresh() {
        let plans = ImportPlans::default();
        let now = Instant::now();
        plans.put("a".into(), a_plan(), now);
        assert!(plans.take("a", now + Duration::from_secs(1)).is_some());
        assert!(plans.take("a", now + Duration::from_secs(2)).is_none());
        plans.put("b".into(), a_plan(), now);
        assert!(
            plans
                .take("b", now + Duration::from_secs(16 * 60))
                .is_none()
        );
        assert!(plans.take("nope", now).is_none());
    }

    #[test]
    fn the_oldest_is_forgotten_when_there_are_too_many() {
        let plans = ImportPlans::default();
        let now = Instant::now();
        for id in ["1", "2", "3", "4", "5"] {
            plans.put(id.into(), a_plan(), now);
        }
        assert!(plans.take("1", now).is_none());
        assert!(plans.take("5", now).is_some());
    }
}
