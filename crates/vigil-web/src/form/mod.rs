//! The form for adding and editing a watch: reading what was typed, checking it, and turning it
//! into a watch.

pub(crate) mod build;
mod draft;

pub(crate) use build::{Built, Known, Problems, build, build_for_preview, preview_watch};
pub(crate) use draft::{Draft, STEP_KINDS, StepDraft, StepOp};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::OwnedMutexGuard;
use vigil_core::WatchId;

/// The fields of a posted form, in order. A name can occur more than once (checkboxes).
#[derive(Debug, Default)]
pub(crate) struct Fields(Vec<(String, String)>);

impl Fields {
    /// Read an `application/x-www-form-urlencoded` body.
    pub(crate) fn parse(body: &str) -> Self {
        Self(
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect(),
        )
    }

    /// The first value of a field, or "" if it was not sent.
    pub(crate) fn get(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|(field, _)| field == name)
            .map_or("", |(_, value)| value.as_str())
    }

    /// Every value of a field.
    pub(crate) fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// Whether a field was sent at all, as a ticked checkbox is.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(field, _)| field == name)
    }

    /// The names of all fields.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }
}

/// A fresh random id for a form being edited, to tell it from every other form.
pub(crate) fn new_draft_id() -> String {
    let mut bytes = [0u8; 8];
    // Without randomness the id is just less unique: the forms of two tabs may then share a
    // processing slot, which only makes one of them wait.
    if getrandom::fill(&mut bytes).is_err() {
        bytes = [0; 8];
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The processing slot of a form: a key of its own that cannot be the id of a watch, so that
/// working on the form never takes the slot a scheduled check of the watch needs.
pub(crate) fn slot_of(draft_id: &str) -> WatchId {
    let hash = blake3::hash(format!("vigil-form:{draft_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    WatchId::from_uuid(uuid::Uuid::from_bytes(bytes))
}

/// Makes work on one form's processing slot take turns.
///
/// The scheduler refuses a second processing thread for a slot while the first is alive,
/// which is right for a check but not for a person: a Save pressed while a preview is still
/// being worked out should wait for it, not fail. Holding the form's lock around such work
/// does that. A thread that really never ends still holds the slot after its caller has gone,
/// and then the refusal is the right answer.
#[derive(Debug, Default)]
pub(crate) struct SlotLocks {
    locks: Mutex<HashMap<WatchId, Arc<tokio::sync::Mutex<()>>>>,
}

/// Locks kept before the ones nobody holds are forgotten.
const KEEP_LOCKS: usize = 128;

impl SlotLocks {
    /// Wait for the turn of `slot`; the turn ends when the guard is dropped.
    pub(crate) async fn hold(&self, slot: WatchId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            if locks.len() >= KEEP_LOCKS {
                // A lock only this map points to is not held and not awaited.
                locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            }
            Arc::clone(locks.entry(slot).or_default())
        };
        lock.lock_owned().await
    }

    /// How many locks are kept.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn work_on_one_slot_takes_turns_and_on_different_slots_does_not() {
        let locks = Arc::new(SlotLocks::default());
        let (a, b) = (slot_of("a"), slot_of("b"));
        let running = Arc::new(AtomicUsize::new(0));
        let overlapped = Arc::new(AtomicUsize::new(0));

        let work = |slot| {
            let (locks, running, overlapped) = (
                Arc::clone(&locks),
                Arc::clone(&running),
                Arc::clone(&overlapped),
            );
            tokio::spawn(async move {
                let _turn = locks.hold(slot).await;
                if running.fetch_add(1, Ordering::SeqCst) > 0 {
                    overlapped.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                running.fetch_sub(1, Ordering::SeqCst);
            })
        };
        let same: Vec<_> = (0..4).map(|_| work(a)).collect();
        for task in same {
            task.await.expect("task");
        }
        assert_eq!(
            overlapped.load(Ordering::SeqCst),
            0,
            "one slot, one at a time"
        );

        let (first, second) = (work(a), work(b));
        let _ = tokio::join!(first, second);
        assert_eq!(overlapped.load(Ordering::SeqCst), 1, "two slots do overlap");
    }

    #[tokio::test]
    async fn locks_nobody_holds_are_forgotten() {
        let locks = SlotLocks::default();
        for n in 0..(KEEP_LOCKS * 2) {
            drop(locks.hold(slot_of(&n.to_string())).await);
        }
        assert!(locks.len() <= KEEP_LOCKS, "{}", locks.len());
        // A lock that is held stays.
        let held = locks.hold(slot_of("held")).await;
        for n in 0..(KEEP_LOCKS * 2) {
            drop(locks.hold(slot_of(&format!("x{n}"))).await);
        }
        assert!(
            locks
                .locks
                .lock()
                .expect("lock")
                .contains_key(&slot_of("held"))
        );
        drop(held);
    }

    #[test]
    fn fields_are_read_in_order_and_decoded() {
        let fields = Fields::parse("a=1&b=two+words&a=3&c&tag=%C3%A9");
        assert_eq!(fields.get("a"), "1");
        assert_eq!(fields.all("a"), ["1", "3"]);
        assert_eq!(fields.get("b"), "two words");
        assert_eq!(fields.get("missing"), "");
        assert!(fields.has("c") && fields.get("c").is_empty());
        assert!(!fields.has("d"));
        assert_eq!(fields.get("tag"), "é");
    }

    #[test]
    fn draft_ids_differ_and_slots_follow_them() {
        let (a, b) = (new_draft_id(), new_draft_id());
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert_eq!(slot_of(&a), slot_of(&a));
        assert_ne!(slot_of(&a), slot_of(&b));
    }
}
