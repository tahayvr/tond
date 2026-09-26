// log_limit.rs — keep the libraries' per-request noise out of the journal
//
// Pingora logs every failed request at ERROR ("Fail to proxy: Upstream
// ConnectRefused …" for each 502 of an app that is down) and every broken
// TLS handshake ("Downstream handshake error" — internet scanners do that all
// day). One stopped app probed by the uptime checks is ~100 lines an hour;
// the budget is 10 lines an hour per service at idle. The failures themselves
// are not lost: every request is counted in the request metrics (502 / 504
// counters, per route) and the uptime checks record the outage.
//
// So WARN/ERROR events from targets that are not Tond's own are rate limited
// per call site (the first one is logged, then at most one per WINDOW), and
// Pingora's final "Fail to proxy" error goes through the same limiter via
// `ProxyHttp::suppress_error_log` (which also saves formatting the request
// summary for the suppressed ones). What was held back is reported in one
// summary line at most once per WINDOW (`take_summary`, called from the
// background loop). Tond's own targets (`tond_*`) are never limited.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::{Context, Filter};

/// At most one line per call site, and one summary, per window.
pub const WINDOW: Duration = Duration::from_secs(3600);
/// Longest example message kept for the summary.
const MAX_EXAMPLE: usize = 200;

/// Key of Pingora's "Fail to proxy" error (logged via the hook, not a call
/// site we see here).
pub const FAIL_TO_PROXY: (&str, u32) = ("pingora_proxy::fail_to_proxy", 0);

#[derive(Default)]
struct Inner {
    last: HashMap<(String, u32), Instant>,
    suppressed: u64,
    example: Option<String>,
    last_summary: Option<Instant>,
}

/// Per-call-site rate limiter.
#[derive(Default)]
pub struct Limiter {
    inner: Mutex<Inner>,
}

pub static LIMITER: LazyLock<Limiter> = LazyLock::new(Limiter::default);

impl Limiter {
    /// May the event at `key` be logged at `now`? When not, it is counted
    /// (with `example` for the summary, only the window's first one is kept).
    pub fn allow(&self, key: (&str, u32), now: Instant, example: impl FnOnce() -> String) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match inner.last.get_mut(&(key.0.to_string(), key.1)) {
            Some(last) if now.duration_since(*last) < WINDOW => {
                inner.suppressed += 1;
                if inner.example.is_none() {
                    let mut text = example();
                    if text.len() > MAX_EXAMPLE {
                        let mut cut = MAX_EXAMPLE;
                        while !text.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        text.truncate(cut);
                        text.push('…');
                    }
                    inner.example = Some(text);
                }
                false
            }
            Some(last) => {
                *last = now;
                true
            }
            None => {
                inner.last.insert((key.0.to_string(), key.1), now);
                true
            }
        }
    }

    /// Lines held back since the last summary, with an example — at most
    /// once per window, and only when something was held back.
    pub fn take_summary(&self, now: Instant) -> Option<(u64, String)> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.suppressed == 0
            || inner
                .last_summary
                .is_some_and(|t| now.duration_since(t) < WINDOW)
        {
            return None;
        }
        inner.last_summary = Some(now);
        let n = std::mem::take(&mut inner.suppressed);
        Some((n, inner.example.take().unwrap_or_default()))
    }
}

/// Only WARN and ERROR events of other crates are limited.
fn limited(meta: &Metadata<'_>) -> bool {
    *meta.level() <= Level::WARN && !meta.target().starts_with("tond")
}

/// The `message` field, for the summary's example.
#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        }
    }
}

/// Per-layer filter for the journal output.
pub struct RateLimit;

impl<S> Filter<S> for RateLimit {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        true
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        // Events bridged from the `log` crate (Pingora) share one call site
        // per level with target "log": decide per event.
        if limited(meta) || meta.target() == "log" {
            Interest::sometimes()
        } else {
            Interest::always()
        }
    }

    fn event_enabled(&self, event: &Event<'_>, _cx: &Context<'_, S>) -> bool {
        let normalized = event.normalized_metadata();
        let meta = normalized.as_ref().unwrap_or_else(|| event.metadata());
        if !limited(meta) {
            return true;
        }
        LIMITER.allow(
            (meta.target(), meta.line().unwrap_or(0)),
            Instant::now(),
            || {
                let mut m = Message::default();
                event.record(&mut m);
                format!("{}: {}", meta.target(), m.0)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_per_site_passes_then_one_per_window() {
        let l = Limiter::default();
        let t0 = Instant::now();
        assert!(l.allow(("pingora_core::x", 10), t0, || "a".into()));
        assert!(
            !l.allow(("pingora_core::x", 10), t0 + Duration::from_secs(1), || "b"
                .into())
        );
        assert!(!l.allow(
            ("pingora_core::x", 10),
            t0 + Duration::from_secs(60),
            || "c".into()
        ));
        // Another call site is independent.
        assert!(
            l.allow(("pingora_core::x", 11), t0 + Duration::from_secs(1), || "d"
                .into())
        );
        assert!(l.allow(("pingora_core::x", 10), t0 + WINDOW, || "e".into()));
    }

    #[test]
    fn summary_reports_what_was_held_back_once_per_window() {
        let l = Limiter::default();
        let t0 = Instant::now();
        assert_eq!(l.take_summary(t0), None);
        l.allow(FAIL_TO_PROXY, t0, || "first".into());
        for i in 0..5 {
            l.allow(FAIL_TO_PROXY, t0 + Duration::from_secs(i + 1), || {
                format!("held back {i}")
            });
        }
        assert_eq!(
            l.take_summary(t0 + Duration::from_secs(10)),
            Some((5, "held back 0".into()))
        );
        // Nothing new, and inside the window: no summary.
        assert_eq!(l.take_summary(t0 + Duration::from_secs(20)), None);
        l.allow(FAIL_TO_PROXY, t0 + Duration::from_secs(30), || {
            "later".into()
        });
        assert_eq!(l.take_summary(t0 + Duration::from_secs(40)), None);
        assert_eq!(
            l.take_summary(t0 + Duration::from_secs(10) + WINDOW),
            Some((1, "later".into()))
        );
    }

    #[test]
    fn long_examples_are_cut_on_a_char_boundary() {
        let l = Limiter::default();
        let t0 = Instant::now();
        l.allow(("x", 1), t0, String::new);
        l.allow(("x", 1), t0, || "é".repeat(300));
        let (_, example) = l.take_summary(t0).unwrap_or_default();
        assert!(example.len() <= MAX_EXAMPLE + '…'.len_utf8());
        assert!(example.ends_with('…'));
    }

    #[test]
    fn only_warnings_and_errors_of_other_crates_are_limited() {
        use tracing_subscriber::prelude::*;

        let buf = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = buf.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || Sink(writer.clone()))
                .with_filter(RateLimit),
        );
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..3 {
                tracing::error!(target: "pingora_test_only", "library noise");
                tracing::warn!(target: "tond_proxy::x", "our own warning");
                tracing::info!(target: "pingora_test_only", "library info");
            }
        });
        let out = String::from_utf8(buf.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default();
        assert_eq!(out.matches("library noise").count(), 1, "{out}");
        assert_eq!(out.matches("our own warning").count(), 3, "{out}");
        assert_eq!(out.matches("library info").count(), 3, "{out}");
    }

    #[test]
    fn records_bridged_from_the_log_crate_are_limited_by_their_real_call_site() {
        use tracing_subscriber::prelude::*;

        // Pingora logs through `log`; the bridge is global (one per process).
        let _ = tracing_log::LogTracer::init();
        let buf = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = buf.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || Sink(writer.clone()))
                .with_filter(RateLimit),
        );
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..3 {
                tracing_log::log::error!(target: "pingora_bridge_test", "bridged noise");
                tracing_log::log::warn!(target: "tond_bridge_test", "bridged own");
            }
        });
        let out = String::from_utf8(buf.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default();
        assert_eq!(out.matches("bridged noise").count(), 1, "{out}");
        assert_eq!(out.matches("bridged own").count(), 3, "{out}");
    }

    struct Sink(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
