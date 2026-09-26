// logging.rs — the log output format of tond-server and tond-proxy
//
// TOND_LOG_FORMAT=text (default) keeps the human-readable lines journald
// shows well. TOND_LOG_FORMAT=json writes one JSON object per line for log
// shippers (Vector, Promtail, Fluent Bit reading the journal):
//
//   {"timestamp":"…","level":"INFO","target":"tond_server::deploy",
//    "message":"…","<field>":…,"span":{"name":"…","<span field>":…}}
//
// Event fields are flattened next to `message`; the current span's fields
// sit under `span`. The level filter is RUST_LOG as before.

use tracing::Subscriber;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

/// Environment variable that picks the format.
pub const LOG_FORMAT_ENV: &str = "TOND_LOG_FORMAT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

impl LogFormat {
    /// Parse a TOND_LOG_FORMAT value; anything but "json" is text.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == "json" => Self::Json,
            _ => Self::Text,
        }
    }

    pub fn from_env() -> Self {
        Self::parse(std::env::var(LOG_FORMAT_ENV).ok().as_deref())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// RUST_LOG, or `info`.
pub fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// The stdout formatting layer for `format`, filtered by RUST_LOG.
pub fn fmt_layer<S>(format: LogFormat) -> Box<dyn Layer<S> + Send + Sync>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fmt_layer_with_writer(format, std::io::stdout)
}

fn fmt_layer_with_writer<S, W>(format: LogFormat, writer: W) -> Box<dyn Layer<S> + Send + Sync>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    match format {
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_filter(env_filter())
            .boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_target(true)
            .with_writer(writer)
            .with_filter(env_filter())
            .boxed(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::prelude::*;

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
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

    #[test]
    fn format_parses_json_and_defaults_to_text() {
        assert_eq!(LogFormat::parse(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some(" JSON ")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("text")), LogFormat::Text);
        assert_eq!(LogFormat::parse(Some("yaml")), LogFormat::Text);
        assert_eq!(LogFormat::parse(None), LogFormat::Text);
    }

    #[test]
    fn json_lines_carry_level_target_message_fields_and_span() {
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::registry()
            .with(fmt_layer_with_writer(LogFormat::Json, move || {
                writer.clone()
            }));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("deploy", app = "web");
            let _guard = span.enter();
            tracing::warn!(attempt = 2, "health check failed");
        });
        let out = String::from_utf8(buf.0.lock().map(|b| b.clone()).unwrap_or_default())
            .unwrap_or_default();
        let line = out.lines().next().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("not one JSON object per line ({e}): {out}"));
        assert_eq!(v["level"], "WARN");
        assert_eq!(v["message"], "health check failed");
        assert_eq!(v["attempt"], 2);
        assert!(v["target"].as_str().is_some_and(|t| t.contains("logging")));
        assert_eq!(v["span"]["name"], "deploy");
        assert_eq!(v["span"]["app"], "web");
        assert!(v["timestamp"].is_string());
    }
}
