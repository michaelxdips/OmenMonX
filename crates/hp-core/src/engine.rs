//! Polling engine: samples thermals, computes target fan % with hysteresis,
//! logs battery alerts. Fan WRITES activate once the ECMMIO path lands (PawnIO
//! signing); today the engine is monitor-only with the write hook stubbed and
//! clearly flagged in status output.

use crate::config::Config;
use crate::sensors::{read_thermal_ec, read_thermal_wmi, ThermalSample};
use hp_ec::ecio::EcIo;

pub struct Engine {
    pub config: Config,
    /// Interior-mutable because poll takes &self (the daemon and GUI hold
    /// one Engine across polls). Single-threaded per Engine instance.
    state: std::cell::RefCell<TargetState>,
}

/// Hysteresis state: the anchor is the temperature the current target was
/// computed from. The target holds until the sensor moves more than
/// `hysteresis_c` away from the anchor — sensor jitter around a curve
/// breakpoint cannot then oscillate the target poll after poll.
#[derive(Default)]
struct TargetState {
    last_target: Option<u8>,
    anchor_temp: Option<f32>,
}

impl TargetState {
    /// Pure transition: feed the latest max(CPU, GPU) temperature, get the
    /// fan target. Testable without firmware.
    fn next(&mut self, temp: Option<f32>, hysteresis_c: u8, cfg: &Config) -> u8 {
        let hyst = hysteresis_c as f32;
        match (temp, self.anchor_temp, self.last_target) {
            // Inside the band: hold.
            (Some(t), Some(anchor), Some(prev)) if (t - anchor).abs() <= hyst => prev,
            // Outside the band (or first reading): recompute and re-anchor.
            (Some(t), ..) => {
                let target = cfg.fan_pct_for(t);
                self.anchor_temp = Some(t);
                self.last_target = Some(target);
                target
            }
            // No readable temperature: hold whatever we had; a first poll
            // with no sensors falls back to the curve floor rather than 0%.
            (None, _, Some(prev)) => prev,
            (None, _, None) => {
                let target = cfg.fan_pct_for(0.0);
                self.last_target = Some(target);
                target
            }
        }
    }
}

pub struct EngineStatus {
    pub sample: ThermalSample,
    pub target_fan_pct: u8,
    pub fan_write_active: bool,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: std::cell::RefCell::new(TargetState::default()),
        }
    }

    /// One poll iteration. Returns status. Fan write hook is gated on the
    /// ECMMIO capability (not yet signed) — target computed but not applied.
    pub fn poll(&self, bios: &hp_wmi::Bios, io: Option<&EcIo>) -> EngineStatus {
        let mut sample = match io {
            Some(io) => read_thermal_ec(io).unwrap_or_else(|| read_thermal_wmi(bios)),
            None => read_thermal_wmi(bios),
        };

        // fill gaps from WMI when EC missing — but only with a value that is
        // actually present; a missing sensor must stay None, not become 0.0 °C
        if sample.cpu.is_none() && sample.wmi_sensors[0] > 0 {
            sample.cpu = Some(sample.wmi_sensors[0] as f32);
        }
        if sample.gpu.is_none() && sample.wmi_sensors[1] > 0 {
            sample.gpu = Some(sample.wmi_sensors[1] as f32);
        }

        // target fan % from the max relevant temp, with hysteresis.
        let temp = match (sample.cpu, sample.gpu) {
            (Some(c), Some(g)) => Some(c.max(g)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        };
        // Engine takes &self (the daemon and GUI share one Engine across
        // polls), so the hysteresis state is interior-mutable like the
        // pre-v0.4 last_target cell was.
        let target = self
            .state
            .borrow_mut()
            .next(temp, self.config.profile.hysteresis_c, &self.config);

        EngineStatus {
            sample,
            target_fan_pct: target,
            // TODO: flip to true when signed PawnIO ECMMIO module ships
            fan_write_active: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default() // curve: 65C->55, 75C->70, hysteresis 3
    }

    #[test]
    fn first_reading_anchors_and_computes() {
        let mut s = TargetState::default();
        assert_eq!(s.next(Some(65.0), 3, &cfg()), 55);
    }

    #[test]
    fn jitter_within_band_holds_target() {
        let mut s = TargetState::default();
        assert_eq!(s.next(Some(65.0), 3, &cfg()), 55);
        // 66, 64: within 3 C of the 65 anchor — must hold 55 even though
        // the raw curve would give 56 / 53.
        assert_eq!(s.next(Some(66.0), 3, &cfg()), 55);
        assert_eq!(s.next(Some(64.0), 3, &cfg()), 55);
    }

    #[test]
    fn beyond_band_recomputes_and_reanchors() {
        let mut s = TargetState::default();
        assert_eq!(s.next(Some(65.0), 3, &cfg()), 55);
        assert_eq!(s.next(Some(69.0), 3, &cfg()), 61); // outside band: moves
        // New anchor is 69; jitter around 69 holds 61.
        assert_eq!(s.next(Some(68.0), 3, &cfg()), 61);
        assert_eq!(s.next(Some(70.0), 3, &cfg()), 61);
        assert_eq!(s.next(Some(72.1), 3, &cfg()), 65); // beyond again: 55+(7.1/10)*15 = 65.65 -> 65
    }

    #[test]
    fn rising_through_the_curve_still_climbs() {
        // Hysteresis must not stall a real temperature climb: each step
        // beyond the band moves the target.
        let mut s = TargetState::default();
        let mut t = 60.0;
        let mut last = s.next(Some(t), 3, &cfg());
        for step in 0..10 {
            t += 4.0; // 4 C steps: always outside the 3 C band
            let next = s.next(Some(t), 3, &cfg());
            assert!(next >= last, "climb must not reverse at step {step}");
            last = next;
        }
        assert_eq!(last, 100, "curve must reach 100% at 100 C");
    }

    #[test]
    fn missing_reading_holds_previous_target() {
        let mut s = TargetState::default();
        assert_eq!(s.next(Some(65.0), 3, &cfg()), 55);
        assert_eq!(s.next(None, 3, &cfg()), 55);
        assert_eq!(s.next(None, 3, &cfg()), 55);
    }

    #[test]
    fn first_poll_with_no_sensors_uses_curve_floor_not_zero() {
        let mut s = TargetState::default();
        assert_eq!(s.next(None, 3, &cfg()), 25); // curve floor at <=40C
    }
}
