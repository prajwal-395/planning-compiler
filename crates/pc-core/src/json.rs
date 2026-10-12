//! Float rounding for `--json` output.
//!
//! The text renderers print every float with `{:.3}`, so `compile`,
//! `frontier`, `impact` and `diff --json` round to the same 3-decimal
//! precision at the serde serialization boundary. Internal computation
//! keeps full `f64` precision: only the emitted JSON is rounded, via
//! `serialize_with` on the float fields of the report structs.

use serde::Serializer;

/// Decimals carried in `--json` output, matching the `{:.3}` text renderer.
pub const JSON_DECIMALS: i32 = 3;

/// Round to [`JSON_DECIMALS`] decimals. Non-finite values pass through
/// untouched (serde_json already refuses to emit them); signed zero
/// normalizes to `0.0` so `-0.0` never leaks into output.
#[must_use]
pub fn round3(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// `serialize_with` for plain `f64` fields in `--json` reports.
pub fn ser_f64_3<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(round3(*value))
}

/// `serialize_with` for `Option<f64>` fields in `--json` reports.
pub fn ser_opt_f64_3<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => serializer.serialize_some(&round3(*v)),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_the_text_renderer() {
        assert_eq!(round3(0.050000000000000044), 0.05);
        assert_eq!(round3(0.09999999999999998), 0.1);
        assert_eq!(round3(0.10833333333333332), 0.108);
        assert_eq!(round3(0.3333333333333333), 0.333);
        assert_eq!(round3(0.6666666666666666), 0.667);
        assert_eq!(round3(12.333333333333334), 12.333);
        assert_eq!(round3(12.0), 12.0);
        assert_eq!(round3(0.0), 0.0);
        assert_eq!(round3(-0.0), 0.0);
        assert!(round3(f64::NAN).is_nan());
    }
}
