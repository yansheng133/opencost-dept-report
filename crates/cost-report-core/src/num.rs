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
    }
}
