//! Pages fetched for the previews of forms, kept so that a preview does not fetch on every
//! keystroke.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use vigil_scheduler::FetchedPage;

/// Forms whose page is kept.
const MAX_ENTRIES: usize = 8;

/// Most bytes of pages kept at once.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// How long a page is reused.
const TTL: Duration = Duration::from_secs(10 * 60);

struct Entry {
    form: String,
    /// What the page was fetched with; a different request is a different page.
    request: String,
    page: FetchedPage,
    fetched: Instant,
}

/// The page each open form last fetched. Small, bounded, and forgetful: the cost of a miss is
/// one fetch.
#[derive(Default)]
pub(crate) struct PreviewCache {
    entries: Mutex<Vec<Entry>>,
}

impl std::fmt::Debug for PreviewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreviewCache")
    }
}

impl PreviewCache {
    /// The page `form` fetched with `request`, and how long ago, if it is still good.
    pub(crate) fn get(
        &self,
        form: &str,
        request: &str,
        now: Instant,
    ) -> Option<(FetchedPage, Duration)> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .iter()
            .find(|entry| entry.form == form && entry.request == request)
            .map(|entry| {
                (
                    entry.page.clone(),
                    now.saturating_duration_since(entry.fetched),
                )
            })
            .filter(|(_, age)| *age < TTL)
    }

    /// Keep the page `form` just fetched with `request`, in place of what it had.
    pub(crate) fn put(&self, form: &str, request: String, page: FetchedPage, now: Instant) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| {
            entry.form != form && now.saturating_duration_since(entry.fetched) < TTL
        });
        entries.push(Entry {
            form: form.to_owned(),
            request,
            page,
            fetched: now,
        });
        // The oldest go first; the page just stored stays, however big.
        while entries.len() > 1
            && (entries.len() > MAX_ENTRIES
                || entries.iter().map(|entry| entry.page.size()).sum::<u64>() > MAX_BYTES)
        {
            entries.remove(0);
        }
    }

    /// How many pages are kept.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetched page of `size` bytes. `FetchedPage` has no public constructor: it comes from a
    /// fetch, so the tests fetch one from a small local server.
    async fn page(size: usize) -> FetchedPage {
        use vigil_core::Schedule;
        use vigil_fetch::{FetchConfig, Fetcher};
        use vigil_scheduler::Checker;
        use vigil_store::{NewWatch, Store};
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("x".repeat(size), "text/plain"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("vigil.db"))
            .await
            .expect("store");
        let checker = Checker::new(
            store.clone(),
            Fetcher::new(FetchConfig::default()).expect("fetcher"),
        );
        let watch = store
            .create_watch(NewWatch::new(server.uri(), Schedule::Interval(60)))
            .await
            .expect("watch");
        checker.fetch_for_preview(&watch).await.expect("page")
    }

    #[tokio::test]
    async fn a_page_is_reused_for_the_same_request_only() {
        let cache = PreviewCache::default();
        let now = Instant::now();
        cache.put("form-a", "https://a.example".into(), page(10).await, now);

        let (found, age) = cache
            .get("form-a", "https://a.example", now + Duration::from_secs(5))
            .expect("hit");
        assert_eq!((found.size(), age), (10, Duration::from_secs(5)));
        assert!(
            cache.get("form-a", "https://b.example", now).is_none(),
            "another request"
        );
        assert!(
            cache.get("form-b", "https://a.example", now).is_none(),
            "another form"
        );
    }

    #[tokio::test]
    async fn a_page_goes_stale() {
        let cache = PreviewCache::default();
        let now = Instant::now();
        cache.put("f", "r".into(), page(1).await, now);
        assert!(
            cache
                .get("f", "r", now + TTL - Duration::from_secs(1))
                .is_some()
        );
        assert!(cache.get("f", "r", now + TTL).is_none());
    }

    #[tokio::test]
    async fn a_form_keeps_one_page_and_the_oldest_forms_are_dropped() {
        let cache = PreviewCache::default();
        let now = Instant::now();
        let one = page(1).await;
        cache.put("f", "first".into(), one.clone(), now);
        cache.put("f", "second".into(), one.clone(), now);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("f", "first", now).is_none());

        for n in 0..(MAX_ENTRIES + 4) {
            cache.put(
                &format!("form-{n}"),
                "r".into(),
                one.clone(),
                now + Duration::from_millis(n as u64),
            );
        }
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.get("form-0", "r", now).is_none(), "the oldest went");
        assert!(
            cache
                .get(&format!("form-{}", MAX_ENTRIES + 3), "r", now)
                .is_some()
        );
    }

    #[tokio::test]
    async fn the_bytes_kept_are_bounded_too() {
        let cache = PreviewCache::default();
        let now = Instant::now();
        let big = page(6 * 1024 * 1024).await;
        for n in 0..8 {
            cache.put(
                &format!("f{n}"),
                "r".into(),
                big.clone(),
                now + Duration::from_millis(n),
            );
        }
        // 32 MiB holds five pages of 6 MiB.
        assert_eq!(cache.len(), 5);
        assert!(cache.get("f7", "r", now).is_some());
    }
}
