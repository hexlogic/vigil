//! Showing times and intervals to people.

use std::time::{Duration, UNIX_EPOCH};

use crate::i18n::Translator;

/// A moment, shown as how long ago it was, with the exact time to hover over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimeView {
    /// "5 minutes ago", "never".
    pub(crate) text: String,
    /// The time in RFC 3339 for the `datetime` attribute; empty for "never".
    pub(crate) iso: String,
    /// The time to read, for the tooltip; empty for "never".
    pub(crate) title: String,
}

/// How long ago `at` was at `now` (both unix milliseconds), in words.
///
/// A moment in the future (a clock that was set back) reads as "just now".
pub(crate) fn when(t: &Translator, at: Option<i64>, now: i64) -> TimeView {
    let Some(at) = at else {
        return TimeView {
            text: t.t("time.never"),
            iso: String::new(),
            title: String::new(),
        };
    };
    let seconds = usize::try_from((now - at).max(0) / 1000).unwrap_or(usize::MAX);
    let text = if seconds < 60 {
        t.t("time.just_now")
    } else if seconds < 3600 {
        t.plural("time.minutes_ago", seconds / 60)
    } else if seconds < 86_400 {
        t.plural("time.hours_ago", seconds / 3600)
    } else {
        t.plural("time.days_ago", seconds / 86_400)
    };
    let moment = UNIX_EPOCH + Duration::from_millis(u64::try_from(at).unwrap_or(0));
    let iso = humantime::format_rfc3339_seconds(moment).to_string();
    let title = iso.replace('T', " ").replace('Z', " UTC");
    TimeView { text, iso, title }
}

/// An interval as it is typed: `30s`, `5m`, `1h 30m`.
pub(crate) fn every(seconds: u64) -> String {
    humantime::format_duration(Duration::from_secs(seconds)).to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::i18n::{I18n, Lang};

    fn translator(lang: Lang) -> Translator {
        Translator::new(Arc::new(I18n::load().expect("locales parse")), lang)
    }

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_moment_is_told_in_the_largest_fitting_unit() {
        let en = translator(Lang::En);
        let at = |ago_seconds: i64| when(&en, Some(NOW - ago_seconds * 1000), NOW).text;
        assert_eq!(at(0), "just now");
        assert_eq!(at(59), "just now");
        assert_eq!(at(60), "1 minute ago");
        assert_eq!(at(125), "2 minutes ago");
        assert_eq!(at(3599), "59 minutes ago");
        assert_eq!(at(3600), "1 hour ago");
        assert_eq!(at(86_399), "23 hours ago");
        assert_eq!(at(86_400), "1 day ago");
        assert_eq!(at(10 * 86_400), "10 days ago");
    }

    #[test]
    fn the_future_and_never_are_handled() {
        let en = translator(Lang::En);
        assert_eq!(when(&en, Some(NOW + 5_000), NOW).text, "just now");
        let never = when(&en, None, NOW);
        assert_eq!(never.text, "never");
        assert!(never.iso.is_empty() && never.title.is_empty());
    }

    #[test]
    fn the_exact_time_is_given_for_hovering() {
        let view = when(&translator(Lang::En), Some(1_800_000_000_000), NOW);
        assert_eq!(view.iso, "2027-01-15T08:00:00Z");
        assert_eq!(view.title, "2027-01-15 08:00:00 UTC");
    }

    #[test]
    fn ukrainian_uses_its_plural_forms() {
        let uk = translator(Lang::Uk);
        let at = |ago_seconds: i64| when(&uk, Some(NOW - ago_seconds * 1000), NOW).text;
        assert_eq!(at(60), "1 хвилину тому");
        assert_eq!(at(180), "3 хвилини тому");
        assert_eq!(at(300), "5 хвилин тому");
        assert_eq!(at(2 * 3600), "2 години тому");
        assert_eq!(at(21 * 86_400), "21 день тому");
        assert_eq!(when(&uk, None, NOW).text, "ніколи");
    }

    #[test]
    fn intervals_read_like_they_are_typed() {
        assert_eq!(every(30), "30s");
        assert_eq!(every(300), "5m");
        assert_eq!(every(5400), "1h 30m");
        assert_eq!(every(86_400), "1day");
    }
}
