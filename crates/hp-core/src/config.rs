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

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, toml::to_string_pretty(self).unwrap_or_default())
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
