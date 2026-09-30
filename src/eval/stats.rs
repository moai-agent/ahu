//! Summary statistics for evaluation reports, and what they refuse to claim.
//!
//! An evaluation over a handful of runs is a small sample, and a bare rate read
//! off one is easy to over-read. Binary outcomes therefore carry a 95% Wilson
//! score interval alongside the rate, because Wilson stays inside `[0, 1]` and
//! keeps a nonzero width when every run passed or every run failed — the two
//! cases where a naive interval collapses to a point and invites exactly the
//! conclusion the sample cannot support.
//!
//! Weighted continuous scores get no interval here. The weights make the
//! per-run value neither binary nor identically distributed across cases, and a
//! normal-approximation interval over such a mean would look like inference
//! without being any. The mean is reported with its sample size instead.

/// Two-sided 95% normal quantile, the `z` in the interval below.
const Z_95: f64 = 1.959_963_984_540_054;

/// A 95% Wilson score interval for a binomial proportion.
///
/// `n` is the number of runs the proportion was taken over, so a caller that
/// excluded runs it could not classify has to exclude them from `n` as well:
/// the interval describes the runs it was given and nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub lower: f64,
    pub upper: f64,
    pub successes: usize,
    pub samples: usize,
}

impl Interval {
    /// The interval for `successes` of `samples`, or `None` when `samples` is 0.
    ///
    /// No observations is not a rate of zero, so an empty sample yields nothing
    /// rather than an interval around a proportion that was never measured.
    pub fn wilson(successes: usize, samples: usize) -> Option<Self> {
        if samples == 0 || successes > samples {
            return None;
        }
        let n = samples as f64;
        let p = successes as f64 / n;
        let z2 = Z_95 * Z_95;
        let denominator = 1.0 + z2 / n;
        let center = (p + z2 / (2.0 * n)) / denominator;
        let spread = (Z_95 / denominator) * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
        Some(Self {
            lower: round4((center - spread).max(0.0)),
            upper: round4((center + spread).min(1.0)),
            successes,
            samples,
        })
    }

    /// The point estimate the interval surrounds.
    pub fn rate(&self) -> f64 {
        round4(self.successes as f64 / self.samples as f64)
    }

    pub fn width(&self) -> f64 {
        self.upper - self.lower
    }

    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "method": "wilson",
            "level": 0.95,
            "lower": self.lower,
            "upper": self.upper,
            "successes": self.successes,
            "samples": self.samples,
        })
    }
}

/// An interval that may have had nothing to measure, as JSON.
pub fn interval_json(interval: Option<Interval>) -> serde_json::Value {
    interval.map_or(serde_json::Value::Null, Interval::to_json)
}

/// Round as `scripts/local_eval.py trend` does, so the two agree on a value.
pub fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// The mean of the observations there are, or `None` when there are none.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(round4(values.iter().sum::<f64>() / values.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_sample_has_no_interval_rather_than_a_rate_of_zero() {
        assert_eq!(Interval::wilson(0, 0), None);
        // More successes than runs is not a sample; it is a bad caller.
        assert_eq!(Interval::wilson(3, 2), None);
    }

    #[test]
    fn all_pass_and_all_fail_keep_a_nonzero_width_inside_the_unit_range() {
        let all_pass = Interval::wilson(4, 4).expect("interval");
        assert_eq!(all_pass.rate(), 1.0);
        assert_eq!(all_pass.upper, 1.0);
        assert!(all_pass.lower > 0.0 && all_pass.lower < 1.0, "{all_pass:?}");
        assert!(all_pass.width() > 0.0);

        let all_fail = Interval::wilson(0, 4).expect("interval");
        assert_eq!(all_fail.rate(), 0.0);
        assert_eq!(all_fail.lower, 0.0);
        assert!(all_fail.upper > 0.0 && all_fail.upper < 1.0, "{all_fail:?}");
        assert!(all_fail.width() > 0.0);
    }

    #[test]
    fn a_smaller_sample_is_a_wider_interval_and_a_single_run_is_nearly_uninformative() {
        let one = Interval::wilson(1, 1).expect("interval");
        let ten = Interval::wilson(10, 10).expect("interval");
        assert!(one.width() > ten.width(), "{one:?} vs {ten:?}");
        // One passing run cannot exclude a coin flip.
        assert!(one.lower < 0.5, "{one:?}");
    }

    #[test]
    fn a_known_wilson_interval_matches_its_textbook_value() {
        // 8 of 10, the standard worked example: about 0.4902 to 0.9433.
        let interval = Interval::wilson(8, 10).expect("interval");
        assert!((interval.lower - 0.4902).abs() < 5e-4, "{interval:?}");
        assert!((interval.upper - 0.9433).abs() < 5e-4, "{interval:?}");
        assert!(interval.lower < interval.rate() && interval.rate() < interval.upper);
    }

    #[test]
    fn the_json_form_is_explicit_about_method_level_and_sample_size() {
        let value = interval_json(Interval::wilson(1, 2));
        assert_eq!(value["method"], "wilson");
        assert_eq!(value["level"], 0.95);
        assert_eq!(value["samples"], 2);
        assert_eq!(value["successes"], 1);
        assert!(interval_json(None).is_null());
    }

    #[test]
    fn means_cover_only_what_was_observed() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[0.0]), Some(0.0));
        assert_eq!(mean(&[1.0, 2.0]), Some(1.5));
        assert_eq!(mean(&[1.0, 1.0, 0.0]), Some(0.6667));
    }
}
