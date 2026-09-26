// traffic.rs — per-route request counters shared by tond-proxy and tond-server
//
// tond-proxy counts every request it answers in fixed per-route structs
// (atomic increments in Pingora's `logging()` hook) and reports cumulative
// totals over the control socket (`traffic` request). tond-server diffs two
// snapshots to get what happened in between, stores per-minute rows and
// derives RPS, status mix, bandwidth and latency percentiles from them.
//
// Latency is a fixed-bucket histogram (LATENCY_BOUNDS_MS upper bounds plus an
// overflow bucket) so it can be summed across routes and time without
// keeping individual samples; percentiles interpolate inside a bucket.

use serde::{Deserialize, Serialize};

/// Upper bounds (inclusive, milliseconds) of the latency buckets. One more
/// bucket collects everything slower than the last bound.
pub const LATENCY_BOUNDS_MS: [u32; 20] = [
    1, 2, 3, 5, 8, 12, 20, 30, 50, 75, 100, 150, 250, 400, 600, 1000, 1500, 2500, 5000, 10_000,
];

/// Number of latency buckets (the bounds plus the overflow bucket).
pub const LATENCY_BUCKETS: usize = LATENCY_BOUNDS_MS.len() + 1;

/// A latency histogram: counts per bucket.
pub type Histogram = [u64; LATENCY_BUCKETS];

/// Bucket index for a duration in microseconds (no allocation, no float).
#[inline]
pub fn bucket_for_micros(micros: u64) -> usize {
    for (i, bound) in LATENCY_BOUNDS_MS.iter().enumerate() {
        if micros <= u64::from(*bound) * 1000 {
            return i;
        }
    }
    LATENCY_BOUNDS_MS.len()
}

/// The `q`-quantile (0 < q ≤ 1) of a histogram in milliseconds, linearly
/// interpolated inside its bucket. The overflow bucket reports the last
/// bound. None when the histogram is empty.
pub fn percentile(hist: &[u64], q: f64) -> Option<f64> {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return None;
    }
    let rank = (q.clamp(0.0, 1.0) * total as f64).max(1.0);
    let mut seen = 0u64;
    for (i, count) in hist.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        if (seen + count) as f64 >= rank {
            let Some(upper) = LATENCY_BOUNDS_MS.get(i) else {
                return Some(f64::from(LATENCY_BOUNDS_MS[LATENCY_BOUNDS_MS.len() - 1]));
            };
            let lower = if i == 0 {
                0.0
            } else {
                f64::from(LATENCY_BOUNDS_MS[i - 1])
            };
            let within = (rank - seen as f64) / *count as f64;
            let value = lower + (f64::from(*upper) - lower) * within;
            return Some((value * 10.0).round() / 10.0);
        }
        seen += count;
    }
    Some(f64::from(LATENCY_BOUNDS_MS[LATENCY_BOUNDS_MS.len() - 1]))
}

/// Cumulative counters of one route (or a difference of two readings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficCounters {
    /// Requests answered (every status, including ones without a response).
    pub requests: u64,
    /// Responses by status class: [1xx, 2xx, 3xx, 4xx, 5xx].
    pub status: [u64; 5],
    /// 502 and 504 responses (also counted in 5xx).
    pub s502: u64,
    pub s504: u64,
    /// Request body bytes read from clients.
    pub bytes_in: u64,
    /// Response body bytes sent to clients.
    pub bytes_out: u64,
    /// Total time from the request head to the end of the response.
    pub latency: Histogram,
    /// Time from choosing the upstream to its response head (requests that
    /// reached an upstream only).
    pub upstream: Histogram,
}

impl TrafficCounters {
    /// What happened between `prev` and `self`. None when any counter went
    /// backwards (the proxy restarted or the route was re-created), in which
    /// case the caller treats `self` as the whole delta.
    pub fn since(&self, prev: &TrafficCounters) -> Option<TrafficCounters> {
        fn sub(a: u64, b: u64) -> Option<u64> {
            a.checked_sub(b)
        }
        let mut out = TrafficCounters {
            requests: sub(self.requests, prev.requests)?,
            s502: sub(self.s502, prev.s502)?,
            s504: sub(self.s504, prev.s504)?,
            bytes_in: sub(self.bytes_in, prev.bytes_in)?,
            bytes_out: sub(self.bytes_out, prev.bytes_out)?,
            ..Default::default()
        };
        for i in 0..5 {
            out.status[i] = sub(self.status[i], prev.status[i])?;
        }
        for i in 0..LATENCY_BUCKETS {
            out.latency[i] = sub(self.latency[i], prev.latency[i])?;
            out.upstream[i] = sub(self.upstream[i], prev.upstream[i])?;
        }
        Some(out)
    }

    /// Add `other` into `self`.
    pub fn add(&mut self, other: &TrafficCounters) {
        self.requests += other.requests;
        self.s502 += other.s502;
        self.s504 += other.s504;
        self.bytes_in += other.bytes_in;
        self.bytes_out += other.bytes_out;
        for i in 0..5 {
            self.status[i] += other.status[i];
        }
        for i in 0..LATENCY_BUCKETS {
            self.latency[i] += other.latency[i];
            self.upstream[i] += other.upstream[i];
        }
    }

    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }
}

/// What a route serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    App,
    Preview,
    Dashboard,
    /// Hosts nothing is routed for (scanners, stale DNS): one shared bucket.
    Unknown,
}

/// One route's cumulative counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteTraffic {
    pub kind: RouteKind,
    /// App or preview id; None for the dashboard and unknown buckets.
    pub id: Option<String>,
    /// Changes whenever the proxy re-creates the counters (route removed and
    /// added again), so a reader never diffs unrelated readings.
    pub generation: u64,
    pub counters: TrafficCounters,
}

/// Reply to a `traffic` request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficSnapshot {
    /// Proxy process id: a new pid means every counter restarted.
    pub pid: u32,
    pub routes: Vec<RouteTraffic>,
}

/// Encode a histogram compactly for storage: LEB128 varints, trailing zero
/// buckets dropped (an idle minute is a handful of bytes).
pub fn encode_histogram(hist: &[u64]) -> Vec<u8> {
    let used = hist.iter().rposition(|c| *c != 0).map_or(0, |i| i + 1);
    let mut out = Vec::with_capacity(used * 2);
    for mut v in hist[..used].iter().copied() {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    out
}

/// Decode what `encode_histogram` wrote. Malformed input decodes to the
/// buckets read so far (stored data is never trusted to be well-formed).
pub fn decode_histogram(bytes: &[u8]) -> Histogram {
    let mut hist = [0u64; LATENCY_BUCKETS];
    let mut idx = 0;
    let mut value = 0u64;
    let mut shift = 0u32;
    for b in bytes {
        if idx >= LATENCY_BUCKETS {
            break;
        }
        if shift < 64 {
            value |= u64::from(b & 0x7f) << shift;
        }
        if b & 0x80 == 0 {
            hist[idx] = value;
            idx += 1;
            value = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    hist
}

// ── Access logs (opt-in per app) ──
//
// tond-proxy appends one line per request to `<data>/access/<app id>.log`
// (directory owned by tond-proxy, 0700). When the file reaches half the
// budget it becomes `<app id>.log.1` (replacing the previous one), so an app
// never uses more than ACCESS_LOG_MAX_BYTES. tond-server (root) reads them.

/// Directory inside the data directory.
pub const ACCESS_LOG_DIR: &str = "access";

/// Most bytes an app's access log uses on disk (current + rotated file).
pub const ACCESS_LOG_MAX_BYTES: u64 = 60 * 1024 * 1024;

/// Size at which the current file is rotated.
pub const ACCESS_LOG_ROTATE_BYTES: u64 = ACCESS_LOG_MAX_BYTES / 2;

/// An id that is safe as a file name (app ids are UUIDs).
pub fn is_safe_log_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// (current, rotated) access log file names for an app.
pub fn access_log_names(app_id: &str) -> (String, String) {
    (format!("{app_id}.log"), format!("{app_id}.log.1"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_ids_must_be_plain_file_names() {
        assert!(is_safe_log_id("0b5c2a3e-1111-4a2b-9c3d-123456789abc"));
        for bad in ["", "../x", "a/b", "a.log", &"x".repeat(65)] {
            assert!(!is_safe_log_id(bad), "{bad}");
        }
        assert_eq!(access_log_names("a").1, "a.log.1");
    }

    use super::*;

    #[test]
    fn buckets_cover_every_duration() {
        assert_eq!(bucket_for_micros(0), 0);
        assert_eq!(bucket_for_micros(1000), 0);
        assert_eq!(bucket_for_micros(1001), 1);
        assert_eq!(bucket_for_micros(49_000), 8);
        assert_eq!(bucket_for_micros(10_000_000), 19);
        assert_eq!(bucket_for_micros(u64::MAX), 20);
    }

    #[test]
    fn percentiles_interpolate_inside_buckets() {
        let mut h = [0u64; LATENCY_BUCKETS];
        assert_eq!(percentile(&h, 0.5), None);
        // 100 requests in (20, 30] ms.
        h[7] = 100;
        assert_eq!(percentile(&h, 0.5), Some(25.0));
        assert_eq!(percentile(&h, 0.99), Some(29.9));
        // One slow outlier past the last bound.
        h[20] = 1;
        assert_eq!(percentile(&h, 1.0), Some(10_000.0));
        assert!(percentile(&h, 0.5).is_some_and(|p| p < 30.0));
        // Single request in the first bucket.
        let mut one = [0u64; LATENCY_BUCKETS];
        one[0] = 1;
        assert_eq!(percentile(&one, 0.95), Some(1.0));
    }

    #[test]
    fn deltas_detect_resets_and_sums_add_up() {
        let mut a = TrafficCounters {
            requests: 10,
            status: [0, 8, 0, 1, 1],
            s502: 1,
            bytes_in: 100,
            bytes_out: 1000,
            ..Default::default()
        };
        a.latency[3] = 10;
        let mut b = a.clone();
        b.requests = 15;
        b.status[1] = 13;
        b.latency[3] = 15;
        let d = b.since(&a).unwrap_or_default();
        assert_eq!(d.requests, 5);
        assert_eq!(d.status, [0, 5, 0, 0, 0]);
        assert_eq!(d.latency[3], 5);
        assert!(a.since(&b).is_none(), "counters went backwards");
        let mut sum = a.clone();
        sum.add(&d);
        assert_eq!(sum, b);
        assert!(TrafficCounters::default().is_empty());
    }

    #[test]
    fn histograms_round_trip_compactly() {
        let mut h = [0u64; LATENCY_BUCKETS];
        assert!(encode_histogram(&h).is_empty());
        assert_eq!(decode_histogram(&[]), h);
        h[0] = 1;
        h[4] = 300;
        h[9] = u64::MAX;
        let bytes = encode_histogram(&h);
        assert!(bytes.len() < 30);
        assert_eq!(decode_histogram(&bytes), h);
        // Garbage never panics.
        let _ = decode_histogram(&[0xff; 100]);
    }

    #[test]
    fn snapshot_serializes_with_snake_case_kinds() {
        let s = TrafficSnapshot {
            pid: 1,
            routes: vec![RouteTraffic {
                kind: RouteKind::Unknown,
                id: None,
                generation: 3,
                counters: TrafficCounters::default(),
            }],
        };
        let json = serde_json::to_string(&s).unwrap_or_default();
        assert!(json.contains("\"kind\":\"unknown\""));
        let back: Option<TrafficSnapshot> = serde_json::from_str(&json).ok();
        assert_eq!(back, Some(s));
    }
}
