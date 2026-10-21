//! Refresh-rate switching via pure Win32.
//!
//! No HP protocol is involved: `EnumDisplaySettingsW` lists the modes the panel
//! and driver actually offer, and `ChangeDisplaySettingsExW` applies one. The
//! trial-then-apply order matters — `CDS_TEST` asks the driver "would this work"
//! without touching the screen, so a mode the driver rejects cannot leave the
//! user staring at a black panel.
//!
//! The list is built from the driver's own enumeration, never from a hardcoded
//! table: a 165 Hz mode that the panel does not support must not be offered.

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_TEST, CDS_TYPE, CDS_UPDATEREGISTRY,
    DEVMODEW, DISP_CHANGE, DISP_CHANGE_BADDUALVIEW, DISP_CHANGE_BADFLAGS,
    DISP_CHANGE_BADMODE, DISP_CHANGE_BADPARAM, DISP_CHANGE_FAILED, DISP_CHANGE_NOTUPDATED,
    DISP_CHANGE_RESTART, DISP_CHANGE_SUCCESSFUL, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};

/// One mode the driver reports for the current resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
    pub bits: u32,
}

impl Mode {
    pub fn label(&self) -> String {
        format!("{}x{} @ {} Hz", self.width, self.height, self.hz)
    }
}

fn devmode_for(m: Mode) -> DEVMODEW {
    let mut dm = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        dmPelsWidth: m.width,
        dmPelsHeight: m.height,
        dmDisplayFrequency: m.hz,
        dmBitsPerPel: m.bits,
        ..Default::default()
    };
    dm.dmFields =
        windows::Win32::Graphics::Gdi::DM_PELSWIDTH
            | windows::Win32::Graphics::Gdi::DM_PELSHEIGHT
            | windows::Win32::Graphics::Gdi::DM_DISPLAYFREQUENCY
            | windows::Win32::Graphics::Gdi::DM_BITSPERPEL;
    dm
}

/// The mode the display is currently running.
pub fn current() -> Option<Mode> {
    unsafe {
        let mut dm = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let ok = EnumDisplaySettingsW(
            PCWSTR::null(),
            ENUM_CURRENT_SETTINGS,
            &mut dm,
        );
        if !ok.as_bool() {
            return None;
        }
        Some(Mode {
            width: dm.dmPelsWidth,
            height: dm.dmPelsHeight,
            hz: dm.dmDisplayFrequency,
            bits: dm.dmBitsPerPel,
        })
    }
}

/// Every refresh rate the driver offers at the current resolution and depth,
/// deduplicated and sorted ascending.
///
/// Restricted to the current width/height/depth on purpose: offering a
/// resolution change here would be a different feature with different failure
/// modes, and this card's whole claim is "same picture, different cadence".
pub fn available_rates() -> Vec<Mode> {
    let Some(cur) = current() else {
        return Vec::new();
    };
    let mut out: Vec<Mode> = Vec::new();
    let mut i: u32 = 0;
    loop {
        let mut dm = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR::null(),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut dm,
            )
        };
        if !ok.as_bool() {
            break;
        }
        i += 1;
        if i > 1024 {
            // A driver that never reports end-of-list must not hang the UI.
            break;
        }
        let m = Mode {
            width: dm.dmPelsWidth,
            height: dm.dmPelsHeight,
            hz: dm.dmDisplayFrequency,
            bits: dm.dmBitsPerPel,
        };
        // 0 and 1 are the "hardware default" pseudo-rates, not real modes.
        if m.hz <= 1 || m.width != cur.width || m.height != cur.height || m.bits != cur.bits {
            continue;
        }
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out.sort_by_key(|m| m.hz);
    out
}

fn describe(r: DISP_CHANGE) -> &'static str {
    match r {
        DISP_CHANGE_SUCCESSFUL => "applied",
        DISP_CHANGE_RESTART => "applied, reboot required",
        DISP_CHANGE_FAILED => "driver refused the mode",
        DISP_CHANGE_BADMODE => "mode not supported by this display",
        DISP_CHANGE_NOTUPDATED => "could not write the registry entry",
        DISP_CHANGE_BADFLAGS => "invalid flags",
        DISP_CHANGE_BADPARAM => "invalid parameter",
        DISP_CHANGE_BADDUALVIEW => "not supported in dual-view",
        _ => "unknown failure",
    }
}

/// Test a mode without applying it. Returns `Ok(())` if the driver would accept
/// it, so the UI can grey out impossible choices before the user clicks.
pub fn test(m: Mode) -> Result<(), String> {
    let dm = devmode_for(m);
    let flags: CDS_TYPE = CDS_TEST;
    let r = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR::null(),
            Some(&dm as *const DEVMODEW),
            None,
            flags,
            None,
        )
    };
    if r == DISP_CHANGE_SUCCESSFUL {
        Ok(())
    } else {
        Err(describe(r).to_string())
    }
}

/// Apply a mode. Runs [`test`] first so a rejected mode never reaches the
/// screen, then applies with `CDS_UPDATEREGISTRY` so the choice survives a
/// reboot.
pub fn apply(m: Mode) -> Result<(), String> {
    test(m)?;
    let dm = devmode_for(m);
    let flags: CDS_TYPE = CDS_UPDATEREGISTRY;
    let r = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR::null(),
            Some(&dm as *const DEVMODEW),
            None,
            flags,
            None,
        )
    };
    if r == DISP_CHANGE_SUCCESSFUL || r == DISP_CHANGE_RESTART {
        Ok(())
    } else {
        Err(describe(r).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devmode_sets_every_field_it_claims() {
        let m = Mode { width: 2560, height: 1440, hz: 165, bits: 32 };
        let dm = devmode_for(m);
        assert_eq!(dm.dmSize as usize, std::mem::size_of::<DEVMODEW>());
        assert_eq!(dm.dmPelsWidth, 2560);
        assert_eq!(dm.dmPelsHeight, 1440);
        assert_eq!(dm.dmDisplayFrequency, 165);
        assert_eq!(dm.dmBitsPerPel, 32);
        // A field that is set in dmFields but not in the struct would be applied
        // as zero, which blanks the display.
        use windows::Win32::Graphics::Gdi::{
            DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH,
        };
        for (name, f) in [
            ("DM_PELSWIDTH", DM_PELSWIDTH),
            ("DM_PELSHEIGHT", DM_PELSHEIGHT),
            ("DM_DISPLAYFREQUENCY", DM_DISPLAYFREQUENCY),
            ("DM_BITSPERPEL", DM_BITSPERPEL),
        ] {
            assert!(dm.dmFields.contains(f), "dmFields missing {name}");
        }
    }

    #[test]
    fn labels_are_stable() {
        let m = Mode { width: 1920, height: 1080, hz: 144, bits: 32 };
        assert_eq!(m.label(), "1920x1080 @ 144 Hz");
    }
}
