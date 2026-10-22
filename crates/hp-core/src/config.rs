//! TOML configuration with serde.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanCurvePoint {
    pub temp_c: u8,
    pub fan_pct: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanProfile {
    pub name: String,
    pub curve: Vec<FanCurvePoint>,
    #[serde(default = "default_hysteresis")]
    pub hysteresis_c: u8,
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u32,
}

fn default_hysteresis() -> u8 { 3 }
fn default_poll_ms() -> u32 { 1000 }

impl Default for FanProfile {
    fn default() -> Self {
        Self {
            name: "balanced".into(),
            hysteresis_c: 3,
            poll_ms: 1000,
            curve: vec![
                FanCurvePoint { temp_c: 40, fan_pct: 25 },
                FanCurvePoint { temp_c: 55, fan_pct: 40 },
                FanCurvePoint { temp_c: 65, fan_pct: 55 },
                FanCurvePoint { temp_c: 75, fan_pct: 70 },
                FanCurvePoint { temp_c: 85, fan_pct: 90 },
                FanCurvePoint { temp_c: 92, fan_pct: 100 },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub profile: FanProfile,
    /// Battery guard: warn when health drops below this
    #[serde(default = "default_health_warn")]
    pub battery_health_warn_pct: u8,
    /// Battery charge alarm: alert at/above this charge while AC (plugged-in wear guard)
    #[serde(default = "default_charge_alert")]
    pub charge_alert_pct: u8,
    #[serde(default)]
    pub enable_charge_alerts: bool,

    // ---- verified control channels ----
    /// CPU power cap to apply at startup, in watts.
    ///
    /// `None` means "leave the firmware alone". The firmware resets to its own
    /// default (25 W on board 8C9C) on every boot, so a persisted value has to be
    /// re-applied explicitly.
    #[serde(default)]
    pub power_cap_watts: Option<u8>,
    /// Thermal mode to apply at startup: "balanced", "performance" or "cool".
    #[serde(default)]
    pub thermal_mode: Option<String>,
}

fn default_health_warn() -> u8 { 70 }
fn default_charge_alert() -> u8 { 95 }

impl Config {
    pub fn path() -> PathBuf {
        dirs_next_home().join(".config").join("omenmon").join("config.toml")
    }

    pub fn load() -> Self {
        let p = Self::path();
        if let Ok(txt) = std::fs::read_to_string(&p) {
            if let Ok(c) = toml::from_str(&txt) {
                return c;
            }
        }
        Self::default()
    }

    /// Load from an explicit path. Used by tests so they never touch the real
    /// user config, and available to any caller that needs to read a config that
    /// is not the active one.
    pub fn load_from(p: &std::path::Path) -> Self {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    /// Write to an explicit path, creating the parent directory.
    pub fn save_to(&self, p: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, toml::to_string_pretty(self).unwrap_or_default())
    }

    /// Parse the persisted thermal mode string.
    ///
    /// Unknown values are treated as "not set" rather than defaulting to a mode,
    /// so a typo in the config file cannot silently change machine behaviour.
    pub fn thermal_mode_parsed(&self) -> Option<&'static str> {
        match self.thermal_mode.as_deref()? {
            "balanced" => Some("balanced"),
            "performance" => Some("performance"),
            "cool" => Some("cool"),
            _ => None,
        }
    }

    /// Persist a power cap. Pass `None` to clear it.
    pub fn set_power_cap(&mut self, watts: Option<u8>) {
        self.power_cap_watts = watts;
    }

    /// Persist a thermal mode by its CLI name. Pass `None` to clear it.
    pub fn set_thermal_mode(&mut self, mode: Option<&str>) {
        self.thermal_mode = mode.map(|s| s.to_string());
    }

    /// Record a control write that the firmware will forget on the next boot.
    ///
    /// Both the GUI and the CLI go through this, which is the point: before it
    /// existed the GUI wrote the hardware and never told the config, so a cap set
    /// in the GUI was invisible to `omenmon apply` and vanished on exit while the
    /// CLI's own cap survived a reboot.
    ///
    /// `None` clears the stored value, because "restore the firmware's own block"
    /// is a state worth remembering too — a saved cap that the user has since
    /// removed must not be re-applied at the next login.
    pub fn record_power_cap(&mut self, watts: Option<u8>) -> std::io::Result<()> {
        self.set_power_cap(watts);
        self.save()
    }

    /// Record a thermal profile write. See [`Config::record_power_cap`].
    pub fn record_thermal_mode(&mut self, mode: Option<&str>) -> std::io::Result<()> {
        self.set_thermal_mode(mode);
        self.save()
    }

    /// Interpolate the curve at a given temperature.
    pub fn fan_pct_for(&self, temp_c: f32) -> u8 {
        let c = &self.profile.curve;
        if c.is_empty() {
            return 50;
        }
        if temp_c <= c[0].temp_c as f32 {
            return c[0].fan_pct;
        }
        for w in c.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let (ta, tb) = (a.temp_c as f32, b.temp_c as f32);
            if temp_c <= tb {
                let t = (temp_c - ta) / (tb - ta).max(0.001);
                return (a.fan_pct as f32 + t * (b.fan_pct - a.fan_pct) as f32) as u8;
            }
        }
        c.last().map(|p| p.fan_pct).unwrap_or(100)
    }
}

fn dirs_next_home() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory per test, so tests never touch the real user config.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "omenmon-config-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// P1-2's contract, at the level the GUI and CLI both rely on: a cap written
    /// through the shared API lands in the file and reads back. The GUI used to
    /// write hardware and skip this entirely, so `omenmon apply` could not see a
    /// cap the user had just set in the UI.
    #[test]
    fn recorded_power_cap_lands_in_the_file() {
        let dir = scratch("cap");
        let p = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.set_power_cap(Some(45));
        cfg.save_to(&p).unwrap();

        let back = Config::load_from(&p);
        assert_eq!(back.power_cap_watts, Some(45));

        // Clearing is a state too: a cap the user removed must not come back at
        // the next login.
        let mut cfg = back;
        cfg.set_power_cap(None);
        cfg.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p).power_cap_watts, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The thermal profile follows the same rule as the cap: a GUI write that has
    /// a config representation must persist it, or the CLI and GUI disagree about
    /// what the machine is set to.
    #[test]
    fn recorded_thermal_mode_round_trips() {
        let dir = scratch("thermal");
        let p = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.set_thermal_mode(Some("performance"));
        cfg.save_to(&p).unwrap();

        let back = Config::load_from(&p);
        assert_eq!(back.thermal_mode_parsed(), Some("performance"));

        let mut cfg = back;
        cfg.set_thermal_mode(Some("cool"));
        cfg.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p).thermal_mode_parsed(), Some("cool"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A saved cap must survive being written next to every other setting, so the
    /// GUI recording a cap cannot drop the user's fan profile.
    #[test]
    fn recording_a_cap_preserves_unrelated_settings() {
        let dir = scratch("preserve");
        let p = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.profile.name = "quiet".into();
        cfg.battery_health_warn_pct = 55;
        cfg.set_power_cap(Some(30));
        cfg.save_to(&p).unwrap();

        let mut back = Config::load_from(&p);
        back.set_power_cap(Some(50));
        back.save_to(&p).unwrap();

        let back = Config::load_from(&p);
        assert_eq!(back.power_cap_watts, Some(50));
        assert_eq!(back.profile.name, "quiet");
        assert_eq!(back.battery_health_warn_pct, 55);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt file must not take the app down; it falls back to defaults.
    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let dir = scratch("corrupt");
        let p = dir.join("config.toml");
        std::fs::write(&p, "this is not toml {{{").unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.power_cap_watts, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unknown thermal mode is "not set", not a guess at Balanced: a typo must
    /// not silently change machine behaviour.
    #[test]
    fn unknown_thermal_mode_is_not_a_mode() {
        let c = Config {
            thermal_mode: Some("turbo".into()),
            ..Default::default()
        };
        assert_eq!(c.thermal_mode_parsed(), None);
    }
}
