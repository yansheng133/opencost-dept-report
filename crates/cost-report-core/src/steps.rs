//! Which sampling steps the usage chart may use, and how much OpenCost may
//! be asked for in a minute.
//!
//! The limits were measured, not guessed:
//!
//! * **No step under 5 minutes.** Prometheus scrapes every 60 s; a
//!   1-minute segment holds one sample and usage comes out as 0, and a
//!   2-minute segment that misses one scrape drops from 66.7 % to 49.9 % —
//!   a dip made by sampling, not by the department.
//! * **At most 288 points per chart.** Every segment makes OpenCost open a
//!   new batch of connections to Prometheus; 24 h at 5 min (288 segments)
//!   takes about 6 s.
//!
//! The whitelist is enforced here, not only by the page's buttons: anyone
//! can call the API directly.

use std::collections::VecDeque;
use std::sync::Mutex;

pub const STEPS: [(&str, f64); 8] = [
    ("5m", 5.0),
    ("10m", 10.0),
    ("15m", 15.0),
    ("30m", 30.0),
    ("1h", 60.0),
    ("3h", 180.0),
    ("6h", 360.0),
    ("1d", 1440.0),
];
pub const MIN_STEP_MINUTES: f64 = 5.0;
pub const MAX_POINTS: f64 = 288.0;

pub fn step_minutes(step: &str) -> Option<f64> {
    STEPS.iter().find(|(s, _)| *s == step).map(|(_, m)| *m)
}

/// Minutes of any `<n>m|h|d` step, including ones configured by the
/// operator outside the whitelist.
pub fn parse_step(step: &str) -> Option<f64> {
    let (num, unit) = step.split_at(step.len().checked_sub(1)?);
    let n: f64 = if num.is_empty() {
        1.0
    } else {
        num.parse().ok()?
    };
    let mult = match unit {
        "m" => 1.0,
        "h" => 60.0,
        "d" => 1440.0,
        _ => return None,
    };
    (n > 0.0).then_some(n * mult)
}

fn candidates(span: f64) -> &'static [&'static str] {
    if span <= 180.0 {
        &["5m", "10m", "15m"]
    } else if span <= 2880.0 {
        &["5m", "10m", "30m", "1h"]
    } else {
        &["1h", "3h", "6h", "1d"]
    }
}

/// Steps offered for a window of `span` minutes, finest first.
pub fn options(span: Option<f64>) -> Vec<&'static str> {
    let Some(span) = span.filter(|s| *s > 0.0) else {
        return Vec::new();
    };
    candidates(span)
        .iter()
        .copied()
        .filter(|s| {
            let m = step_minutes(s).unwrap_or(0.0);
            // At least two points to draw a line; too many and OpenCost fails.
            m >= MIN_STEP_MINUTES && (2.0..=MAX_POINTS).contains(&(span / m))
        })
        .collect()
}

/// The step used when none is asked for. A configured value is used even
/// outside the whitelist: an operator set it on purpose. Users picking
/// from the page are limited to [`options`].
pub fn default_step(span: Option<f64>, configured: Option<&str>) -> Option<String> {
    if let Some(c) = configured {
        return Some(c.to_string());
    }
    let span = span?;
    Some(
        if span <= 180.0 {
            "5m"
        } else if span <= 2880.0 {
            "10m"
        } else if span <= 14_400.0 {
            "1h"
        } else {
            "1d"
        }
        .into(),
    )
}

pub fn is_allowed(span: Option<f64>, step: &str) -> bool {
    options(span).contains(&step)
}

/// How many segments user-chosen series may ask OpenCost for in a period.
///
/// OpenCost opens new connections to Prometheus for every segment, and a
/// closed port waits out TIME_WAIT (60 s on Linux) before reuse. Two fine
/// queries in a row ran it out of ports (`connect: cannot assign requested
/// address`), and the billing report failed at the same moment. Each query
/// alone was valid, so the whitelist cannot catch it; the total over time
/// can. The background refresh does not go through here: it is a fixed,
/// measured load.
pub struct Budget {
    limit: f64,
    period: f64,
    spent: Mutex<VecDeque<(f64, f64)>>,
}

impl Budget {
    pub fn new(limit: f64, period: f64) -> Self {
        Self {
            limit,
            period,
            spent: Mutex::new(VecDeque::new()),
        }
    }

    /// `Ok(())` and recorded, or `Err(seconds to wait)`.
    pub fn try_take(&self, n: f64, now: f64) -> Result<(), u64> {
        let mut spent = self.spent.lock().unwrap_or_else(|e| e.into_inner());
        while spent.front().is_some_and(|(t, _)| now - t >= self.period) {
            spent.pop_front();
        }
        let used: f64 = spent.iter().map(|(_, k)| k).sum();
        // The first query always passes: whether one query is sensible is
        // the whitelist's call, not this one's.
        if used > 0.0 && used + n > self.limit {
            let (mut free, mut wait) = (self.limit - used, 0.0);
            for (t, k) in spent.iter() {
                free += k;
                wait = t + self.period - now;
                if free >= n {
                    break;
                }
            }
            return Err((wait.ceil() as u64).max(1));
        }
        spent.push_back((now, n));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_by_span() {
        assert_eq!(options(Some(60.0)), ["5m", "10m", "15m"]);
        assert_eq!(options(Some(1440.0)), ["5m", "10m", "30m", "1h"]);
        assert_eq!(options(Some(10_080.0)), ["1h", "3h", "6h", "1d"]);
        // 1440 minutes at 5 min is exactly 288: allowed. 7 days at 30 min is not offered.
        assert!(is_allowed(Some(1440.0), "5m"));
        assert!(!is_allowed(Some(10_080.0), "30m"));
        assert!(!is_allowed(Some(1440.0), "1m"));
        assert!(options(None).is_empty());
        assert!(options(Some(0.0)).is_empty());
        // Too short for two points.
        assert!(!is_allowed(Some(20.0), "15m"));
    }

    #[test]
    fn defaults() {
        assert_eq!(default_step(Some(60.0), None).as_deref(), Some("5m"));
        assert_eq!(default_step(Some(1440.0), None).as_deref(), Some("10m"));
        assert_eq!(default_step(Some(10_080.0), None).as_deref(), Some("1h"));
        assert_eq!(default_step(Some(43_200.0), None).as_deref(), Some("1d"));
        assert_eq!(default_step(Some(60.0), Some("2m")).as_deref(), Some("2m"));
        assert_eq!(default_step(None, None), None);
        assert_eq!(parse_step("2m"), Some(2.0));
        assert_eq!(parse_step("h"), Some(60.0));
        assert_eq!(parse_step("1d"), Some(1440.0));
        assert_eq!(parse_step("5x"), None);
        assert_eq!(parse_step(""), None);
    }

    #[test]
    fn budget() {
        let b = Budget::new(300.0, 60.0);
        assert!(b.try_take(288.0, 0.0).is_ok());
        let wait = b.try_take(288.0, 10.0).unwrap_err();
        assert_eq!(wait, 50);
        assert!(b.try_take(288.0, 60.0).is_ok());
        // A first oversized query still passes.
        let b = Budget::new(100.0, 60.0);
        assert!(b.try_take(288.0, 0.0).is_ok());
        assert!(b.try_take(1.0, 1.0).is_err());
    }
}
