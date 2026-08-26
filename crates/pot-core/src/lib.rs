//! Pure logic for pico-pot-meter: no HAL, no hardware, no_std-compatible.
//! Everything here takes plain numbers in and returns plain numbers out,
//! so it's testable with `cargo test` on the host — no board required.
#![cfg_attr(not(test), no_std)]
/// Map a raw ADC reading to a 0-100 percentage, given calibrated min/max
/// bounds. Handles both wiring directions:
/// - `min < max`: normal pot, raw counts increase as % increases.
/// - `min > max`: inverted pot (wired backwards, or CCW = 100%) — this is
///   just `min`/`max` swapped from the caller's point of view, and the
///   math below handles it without a separate code path.
///   Always clamps to 0..=100, so a raw reading outside the calibrated
///   range (noise, or calibration that didn't quite reach the physical
///   stop) never produces a nonsense percentage.
pub fn raw_to_percent(raw: u16, min: u16, max: u16) -> u8 {
    if min == max {
        return 0;
    }

    let (low, high, inv) = if min < max {
        (min, max, false)
    } else {
        (max, min, true)
    };
    let clamped = raw.clamp(low, high);
    let span = (high - low) as u32;
    let offset = (clamped - low) as u32;
    let res = (offset * 100) / span;
    let pct = if inv { 100 - res } else { res };

    pct as u8
}

/// Simple exponential moving average filter for smoothing ADC noise.
/// `alpha` is the weight given to each new sample (0.0..=1.0) — lower
/// values smooth harder but respond to real changes more slowly.
#[derive(Copy, Clone)]
pub struct Ema {
    alpha: f32,
    value: Option<f32>,
}

impl Ema {
    pub const fn new(alpha: f32) -> Self {
        Self { alpha, value: None }
    }

    pub fn update(&mut self, sample: f32) -> f32 {
        let filtered = match self.value {
            None => sample, //first value , nothing to average against
            Some(prev) => prev + self.alpha * (sample - prev),
        };
        self.value = Some(filtered);
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midpoint_maps_to_fifty_percent() {
        assert_eq!(raw_to_percent(2048, 0, 4095), 50);
    }

    #[test]
    fn clamps_below_calibrated_min() {
        // raw below `min` (e.g. pot settled slightly past where calibration
        // caught it) must still report 0, not wrap or underflow.
        assert_eq!(raw_to_percent(0, 100, 4095), 0);
    }

    #[test]
    fn clamps_above_calibrated_max() {
        assert_eq!(raw_to_percent(4095, 0, 4000), 100);
    }

    #[test]
    fn inverted_pot_at_raw_min_reads_100_percent() {
        // min > max signals the pot is wired/read backwards: the raw
        // minimum should now correspond to 100%, not 0%.
        assert_eq!(raw_to_percent(0, 4095, 0), 100);
    }

    #[test]
    fn inverted_pot_at_raw_max_reads_0_percent() {
        assert_eq!(raw_to_percent(4095, 4095, 0), 0);
    }

    #[test]
    fn degenerate_calibration_reports_zero_not_panic() {
        assert_eq!(raw_to_percent(2048, 100, 100), 0);
    }

    #[test]
    fn ema_converges_toward_a_constant_input() {
        let mut ema = Ema::new(0.2);
        let mut last = ema.update(0.0);
        for _ in 0..50 {
            last = ema.update(100.0);
        }
        assert!(
            (last - 100.0).abs() < 1.0,
            "expected convergence near 100, got {last}"
        );
    }
}
