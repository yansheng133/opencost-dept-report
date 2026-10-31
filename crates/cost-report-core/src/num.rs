//! Reading numbers out of loosely typed JSON, and rounding for display.

use serde_json::Value;

/// A number from JSON; strings of numbers count, anything else is 0.
pub fn f(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `f` of a field.
pub fn field(v: &Value, key: &str) -> f64 {
    f(v.get(key))
}

/// Round to `n` decimals. Non-finite values become 0 so they never reach
/// the JSON encoder (which would refuse them).
pub fn round(v: f64, n: i32) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let p = 10f64.powi(n);
    let r = (v * p).round() / p;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

pub fn r4(v: f64) -> f64 {
    round(v, 4)
}

/// A CPU usage average from OpenCost, per minute actually covered.
///
/// OpenCost divides CPU usage by the length of the window it was asked for,
/// not by the minutes the allocation covers (its request average and RAM
/// figures do use the covered minutes). With 290 minutes of data, "24h"
/// reported a quarter of the usage of the same 290 minutes asked as one
/// day, and efficiencies with it. `window_minutes` is the length asked for.
pub fn cpu_usage(v: &Value, window_minutes: Option<f64>) -> f64 {
    let usage = field(v, "cpuCoreUsageAverage");
    let minutes = field(v, "minutes");
    match window_minutes {
        Some(w) if minutes > 0.0 && w > minutes => usage * w / minutes,
        _ => usage,
    }
}

/// CPU efficiency from [`cpu_usage`] and the request average; `None`
/// without a request.
pub fn cpu_efficiency(v: &Value, window_minutes: Option<f64>) -> Option<f64> {
    let req = field(v, "cpuCoreRequestAverage");
    (req > 0.0).then(|| cpu_usage(v, window_minutes) / req)
}

pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
pub const MIB: f64 = 1024.0 * 1024.0;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_and_rounds() {
        assert_eq!(f(Some(&json!(1.5))), 1.5);
        assert_eq!(f(Some(&json!("2.25"))), 2.25);
        assert_eq!(f(Some(&json!(null))), 0.0);
        assert_eq!(f(None), 0.0);
        assert_eq!(round(1.23456, 2), 1.23);
        assert_eq!(round(f64::NAN, 2), 0.0);
        assert_eq!(round(-0.00001, 2).to_string(), "0");
        // Measured: 290 covered minutes asked as 1440 and as 370 minutes.
        let a =
            json!({"minutes": 290.0, "cpuCoreUsageAverage": 0.0024, "cpuCoreRequestAverage": 0.1});
        let b =
            json!({"minutes": 290.0, "cpuCoreUsageAverage": 0.0095, "cpuCoreRequestAverage": 0.1});
        assert_eq!(
            round(cpu_usage(&a, Some(1440.0)), 3),
            round(cpu_usage(&b, Some(370.0)), 3)
        );
        assert_eq!(round(cpu_efficiency(&a, Some(1440.0)).unwrap(), 2), 0.12);
        assert_eq!(cpu_usage(&a, None), 0.0024);
        assert!(cpu_efficiency(&json!({"cpuCoreUsageAverage": 1.0}), None).is_none());
    }
}
