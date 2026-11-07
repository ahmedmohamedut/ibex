//! Apple timestamp conversion.
//!
//! Apple's Core Data / Cocoa epoch is 2001-01-01T00:00:00Z, which is Unix
//! `978_307_200`. Values appear as seconds (integer or float) in most Core Data
//! databases, and as nanoseconds (integer) in `sms.db` on iOS 11+.
//!
//! **This module is the only place that conversion happens.** See AGENTS.md
//! hard rule 7.

use chrono::{DateTime, TimeZone, Utc};

/// Unix timestamp of 2001-01-01T00:00:00Z.
pub const APPLE_EPOCH_UNIX: i64 = 978_307_200;

/// Above this magnitude a value is read as nanoseconds rather than seconds.
///
/// 1e12 seconds after 2001 is the year 33,658; 1e12 nanoseconds is 16 minutes.
/// Nothing real lands near the boundary, so the heuristic is safe.
const NANOSECOND_THRESHOLD: i64 = 1_000_000_000_000;

/// An Apple timestamp as it comes out of SQLite: either an integer or a float.
///
/// Integers keep `i64` precision deliberately — nanosecond timestamps exceed
/// the range `f64` can represent exactly, so routing them through `f64` would
/// quietly lose sub-second accuracy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppleTs {
    Int(i64),
    Float(f64),
}

impl From<i64> for AppleTs {
    fn from(v: i64) -> Self {
        AppleTs::Int(v)
    }
}

impl From<i32> for AppleTs {
    fn from(v: i32) -> Self {
        AppleTs::Int(i64::from(v))
    }
}

impl From<f64> for AppleTs {
    fn from(v: f64) -> Self {
        AppleTs::Float(v)
    }
}

impl From<f32> for AppleTs {
    fn from(v: f32) -> Self {
        AppleTs::Float(f64::from(v))
    }
}

/// Convert an Apple timestamp to UTC.
///
/// Returns `None` for `0` (Apple's "not set"), for non-finite floats, and for
/// values that fall outside the range `chrono` can represent.
///
/// ```
/// use ibex_core::util::time::from_apple;
///
/// // seconds since 2001-01-01
/// let t = from_apple(0_i64);
/// assert!(t.is_none(), "0 means 'not set'");
/// let t = from_apple(1_i64).unwrap();
/// assert_eq!(t.timestamp(), 978_307_201);
/// ```
pub fn from_apple(value: impl Into<AppleTs>) -> Option<DateTime<Utc>> {
    match value.into() {
        AppleTs::Int(0) => None,
        AppleTs::Int(v) => {
            // `unsigned_abs`, not `abs`: `i64::MIN.abs()` overflows and would
            // panic in debug builds on a corrupt column.
            if v.unsigned_abs() > NANOSECOND_THRESHOLD as u64 {
                let secs = v.div_euclid(1_000_000_000);
                // div_euclid/rem_euclid keep the remainder non-negative, which
                // is what chrono wants even for pre-2001 (negative) values.
                let nanos = v.rem_euclid(1_000_000_000) as u32;
                from_apple_parts(secs, nanos)
            } else {
                from_apple_parts(v, 0)
            }
        }
        AppleTs::Float(v) => {
            if v == 0.0 || !v.is_finite() {
                return None;
            }
            let v = if v.abs() > NANOSECOND_THRESHOLD as f64 {
                v / 1e9
            } else {
                v
            };
            let secs = v.floor();
            // `as i64` saturates on overflow in Rust, and from_apple_parts
            // range-checks the result, so a wild value yields None.
            let nanos = ((v - secs) * 1e9).round() as u32;
            from_apple_parts(secs as i64, nanos.min(999_999_999))
        }
    }
}

/// Seconds (relative to the Apple epoch) plus nanoseconds, to UTC.
fn from_apple_parts(secs: i64, nanos: u32) -> Option<DateTime<Utc>> {
    let unix = secs.checked_add(APPLE_EPOCH_UNIX)?;
    Utc.timestamp_opt(unix, nanos).single()
}

/// Convert a plain **Unix** timestamp in seconds.
///
/// Not everything in a backup uses the Apple epoch: the `MBFile` metadata in
/// `Manifest.db` stores `LastModified`, `Birth` and `LastStatusChange` as
/// ordinary Unix times, because they come straight from the filesystem rather
/// than from Core Data. Those go through here; everything Core Data touches
/// goes through [`from_apple`]. Keeping both in this module means date handling
/// still lives in exactly one place (AGENTS.md hard rule 7).
///
/// Returns `None` for `0`, Apple's and the filesystem's shared "not set".
pub fn from_unix_secs(secs: i64) -> Option<DateTime<Utc>> {
    if secs == 0 {
        return None;
    }
    Utc.timestamp_opt(secs, 0).single()
}

/// Convert a UTC time back to Apple seconds. Used by exporters and by the test
/// fixture builder.
pub fn to_apple_secs(t: DateTime<Utc>) -> f64 {
    (t.timestamp() - APPLE_EPOCH_UNIX) as f64 + f64::from(t.timestamp_subsec_nanos()) / 1e9
}

/// Convert a UTC time to Apple nanoseconds, the `sms.db` form on iOS 11+.
pub fn to_apple_nanos(t: DateTime<Utc>) -> i64 {
    (t.timestamp() - APPLE_EPOCH_UNIX) * 1_000_000_000 + i64::from(t.timestamp_subsec_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn ymd_hms(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, m, d)
            .and_then(|d| d.and_hms_opt(h, mi, s))
            .expect("valid test date")
            .and_utc()
    }

    #[test]
    fn zero_and_null_are_none() {
        assert_eq!(from_apple(0_i64), None);
        assert_eq!(from_apple(0.0_f64), None);
        // A NULL column is an Option at the call site; this documents the
        // intended pattern.
        let column: Option<i64> = None;
        assert_eq!(column.and_then(from_apple), None);
    }

    #[test]
    fn int_seconds() {
        // 2001-01-01 plus one day
        assert_eq!(
            from_apple(86_400_i64).unwrap(),
            ymd_hms(2001, 1, 2, 0, 0, 0)
        );
    }

    #[test]
    fn float_seconds_keep_the_fraction() {
        let t = from_apple(86_400.5_f64).unwrap();
        assert_eq!(t.timestamp(), APPLE_EPOCH_UNIX + 86_400);
        assert_eq!(t.timestamp_subsec_millis(), 500);
    }

    #[test]
    fn int_nanoseconds() {
        // sms.db form: 2001-01-02T00:00:00Z with a 250ms fraction
        let ns = 86_400_i64 * 1_000_000_000 + 250_000_000;
        let t = from_apple(ns).unwrap();
        assert_eq!(t.timestamp(), APPLE_EPOCH_UNIX + 86_400);
        assert_eq!(t.timestamp_subsec_millis(), 250);
    }

    #[test]
    fn nanosecond_threshold_boundary() {
        // Exactly at the threshold is still seconds...
        let secs = NANOSECOND_THRESHOLD;
        assert_eq!(
            from_apple(secs).unwrap().timestamp(),
            APPLE_EPOCH_UNIX + secs
        );
        // ...one above it flips to nanoseconds.
        let t = from_apple(NANOSECOND_THRESHOLD + 1).unwrap();
        assert_eq!(t.timestamp(), APPLE_EPOCH_UNIX + 1_000);
    }

    #[test]
    fn negative_means_before_2001() {
        let t = from_apple(-86_400_i64).unwrap();
        assert_eq!(t, ymd_hms(2000, 12, 31, 0, 0, 0));
    }

    #[test]
    fn negative_nanoseconds_round_down_not_toward_zero() {
        let ns = -(86_400_i64 * 1_000_000_000) - 500_000_000;
        let t = from_apple(ns).unwrap();
        // 2000-12-30T23:59:59.5Z
        assert_eq!(t.timestamp(), APPLE_EPOCH_UNIX - 86_401);
        assert_eq!(t.timestamp_subsec_millis(), 500);
    }

    #[test]
    fn unix_seconds_are_not_apple_seconds() {
        // MBFile's LastModified: 2026-11-01T00:00:00Z as a Unix timestamp.
        let t = from_unix_secs(1_793_491_200).unwrap();
        assert_eq!(t.to_rfc3339(), "2026-11-01T00:00:00+00:00");
        // Read through the Apple-epoch helper the same number would be 2056,
        // which is exactly the 31-year mistake this split prevents.
        assert_eq!(
            from_apple(1_793_491_200_i64)
                .unwrap()
                .format("%Y")
                .to_string(),
            "2057"
        );
        assert_eq!(from_unix_secs(0), None);
    }

    #[test]
    fn non_finite_floats_are_none() {
        assert_eq!(from_apple(f64::NAN), None);
        assert_eq!(from_apple(f64::INFINITY), None);
        assert_eq!(from_apple(f64::NEG_INFINITY), None);
    }

    #[test]
    fn extreme_values_do_not_panic() {
        // Read as nanoseconds, i64's extremes land in years 2293 and -290:
        // implausible, but representable, so they come back as dates rather
        // than being silently dropped. Only "not set" (0/NULL) maps to None;
        // garbage is better surfaced than hidden.
        assert!(from_apple(i64::MAX).is_some());
        assert!(from_apple(i64::MIN).is_some());
        // Floats beyond chrono's range have nowhere to go.
        assert_eq!(from_apple(1e300_f64), None);
        assert_eq!(from_apple(-1e300_f64), None);
    }

    #[test]
    fn round_trips() {
        let t = ymd_hms(2026, 3, 3, 14, 30, 15);
        assert_eq!(from_apple(to_apple_secs(t)).unwrap(), t);
        assert_eq!(from_apple(to_apple_nanos(t)).unwrap(), t);
    }
}
