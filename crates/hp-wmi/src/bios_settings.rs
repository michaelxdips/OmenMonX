//! HP InstrumentedBIOS settings channel (`ROOT\HP\InstrumentedBIOS`).
//!
//! Verified on board 8C9C (2026-10-21). Two things matter:
//!
//! 1. **Writes must be invoked on the INSTANCE, not the class.** A class-level
//!    `ExecMethod` returns `rc=5` for every setting and looks exactly like a
//!    permissions wall. It is not — it is the wrong invocation shape. This module
//!    resolves `HPBIOS_BIOSSettingInterface` to its instance path first.
//!
//! 2. **The return codes are meaningful** and were calibrated against a setting
//!    known to be writable:
//!
//!    | rc | meaning                                      |
//!    |----|----------------------------------------------|
//!    | 0  | accepted                                     |
//!    | 1  | no such setting name                         |
//!    | 5  | name exists, value rejected                  |
//!    | 6  | accepted, reboot required                    |
//!
//!    Without this table a caller cannot tell "wrong value" from "no such
//!    setting", which is why the battery-care question took so long to settle.

use std::fmt;

use windows::core::{BSTR, VARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoInitializeSecurity, CoSetProxyBlanket, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::VariantClear;
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WbemContext,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
};

const NAMESPACE: &str = r"ROOT\HP\InstrumentedBIOS";
const IFACE_CLASS: &str = "HPBIOS_BIOSSettingInterface";
const ENUM_CLASS: &str = "HPBIOS_BIOSEnumeration";
const STRING_CLASS: &str = "HPBIOS_BIOSString";

/// Outcome of a `SetBIOSSetting` call, decoded from the calibrated table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResult {
    /// rc 0 — the firmware accepted the value.
    Accepted,
    /// rc 6 — accepted, but the change only takes effect after a reboot.
    AcceptedNeedsReboot,
    /// rc 1 — no setting with that name exists.
    UnknownSetting,
    /// rc 5 — the name exists but this value was rejected. This is the code that
    /// `Battery Care Function` returns for every value on board 8C9C.
    ValueRejected,
    /// Any other code, passed through untouched.
    Other(u32),
}

impl SetResult {
    fn from_code(code: u32) -> Self {
        match code {
            0 => SetResult::Accepted,
            6 => SetResult::AcceptedNeedsReboot,
            1 => SetResult::UnknownSetting,
            5 => SetResult::ValueRejected,
            other => SetResult::Other(other),
        }
    }

    /// True if the firmware took the value.
    pub fn is_ok(self) -> bool {
        matches!(self, SetResult::Accepted | SetResult::AcceptedNeedsReboot)
    }
}

impl fmt::Display for SetResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetResult::Accepted => write!(f, "accepted"),
            SetResult::AcceptedNeedsReboot => write!(f, "accepted (reboot required)"),
            SetResult::UnknownSetting => write!(f, "no such setting"),
            SetResult::ValueRejected => write!(f, "value rejected by firmware"),
            SetResult::Other(c) => write!(f, "unexpected code {c}"),
        }
    }
}

/// One enumerable BIOS setting.
#[derive(Debug, Clone)]
pub struct BiosSetting {
    /// Setting name, as used by `SetBIOSSetting`.
    pub name: String,
    /// Current value, as a string.
    pub current: String,
    /// Values the firmware advertises as valid.
    pub possible: Vec<String>,
    /// Whether the firmware marks the setting read-only.
    pub read_only: bool,
}

/// A connection to the InstrumentedBIOS namespace.
pub struct BiosSettings {
    services: IWbemServices,
    /// The interface instance path. Writes must target this, not the class.
    instance_path: String,
    ctx: IWbemContext,
}

impl BiosSettings {
    /// Connect and resolve the interface instance.
    pub fn connect() -> windows::core::Result<Self> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            // S_FALSE (0x1) means COM was already initialised on this thread.
            if hr.is_err() && hr.0 != 1 {
                return Err(hr.ok().unwrap_err());
            }
            let _ = CoInitializeSecurity(
                None,
                -1,
                None,
                None,
                windows::Win32::System::Com::RPC_C_AUTHN_LEVEL_DEFAULT,
                windows::Win32::System::Com::RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                windows::Win32::System::Com::EOAC_NONE,
                None,
            );

            let locator: IWbemLocator = CoCreateInstance(
                &windows::core::GUID::from_u128(0x4590F811_1D3A_11D0_891F_00AA004B2E24),
                None,
                CLSCTX_INPROC_SERVER,
            )?;

            let services = locator.ConnectServer(
                &BSTR::from(NAMESPACE),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )?;

            CoSetProxyBlanket(
                &services,
                windows::Win32::System::Rpc::RPC_C_AUTHN_WINNT,
                windows::Win32::System::Rpc::RPC_C_AUTHZ_NONE,
                None,
                windows::Win32::System::Com::RPC_C_AUTHN_LEVEL_CALL,
                windows::Win32::System::Com::RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                windows::Win32::System::Com::EOAC_NONE,
            )?;

            let ctx: IWbemContext = CoCreateInstance(&WbemContext, None, CLSCTX_INPROC_SERVER)?;

            // Resolve the instance path. Without this, every write returns rc=5.
            let query = BSTR::from(format!("SELECT * FROM {IFACE_CLASS}"));
            let enumerator = services.ExecQuery(
                &BSTR::from("WQL"),
                &query,
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                Some(&ctx),
            )?;

            let mut instance_path = String::new();
            loop {
                let mut objs = [None; 1];
                let mut returned = 0;
                enumerator.Next(WBEM_INFINITE, &mut objs, &mut returned).ok();
                if returned == 0 {
                    break;
                }
                if let Some(obj) = &objs[0] {
                    let mut pv = VARIANT::default();
                    if obj.Get(&BSTR::from("__PATH"), 0, &mut pv, None, None).is_ok() {
                        instance_path = pv.to_string();
                        let _ = VariantClear(&mut pv);
                    }
                }
            }
            if instance_path.is_empty() {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "HPBIOS_BIOSSettingInterface instance not found",
                ));
            }

            Ok(Self {
                services,
                instance_path,
                ctx,
            })
        }
    }

    /// The resolved interface instance path.
    pub fn instance_path(&self) -> &str {
        &self.instance_path
    }

    /// Enumerate every readable setting.
    pub fn list(&self) -> windows::core::Result<Vec<BiosSetting>> {
        let mut out = Vec::new();
        for class in [ENUM_CLASS, STRING_CLASS] {
            unsafe {
                let query = BSTR::from(format!("SELECT * FROM {class}"));
                let enumerator = self.services.ExecQuery(
                    &BSTR::from("WQL"),
                    &query,
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    Some(&self.ctx),
                )?;
                loop {
                    let mut objs = [None; 1];
                    let mut returned = 0;
                    enumerator.Next(WBEM_INFINITE, &mut objs, &mut returned).ok();
                    if returned == 0 {
                        break;
                    }
                    let Some(obj) = &objs[0] else { continue };
                    let name = get_string(obj, "Name");
                    if name.is_empty() {
                        continue;
                    }
                    out.push(BiosSetting {
                        current: get_string(obj, "CurrentValue"),
                        possible: get_string_array(obj, "PossibleValues"),
                        read_only: get_bool(obj, "IsReadOnly"),
                        name,
                    });
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Write a setting. Targets the **instance**, which is the part that works.
    pub fn set(&self, name: &str, value: &str) -> windows::core::Result<SetResult> {
        unsafe {
            let mut class: Option<IWbemClassObject> = None;
            self.services.GetObject(
                &BSTR::from(IFACE_CLASS),
                windows::Win32::System::Wmi::WBEM_FLAG_RETURN_WBEM_COMPLETE,
                Some(&self.ctx),
                Some(&mut class),
                None,
            )?;
            let class = class.ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "interface class unavailable",
                )
            })?;

            let mut in_sig = None;
            class.GetMethod(
                &BSTR::from("SetBIOSSetting"),
                0,
                &mut in_sig,
                std::ptr::null_mut(),
            )?;
            let in_sig = in_sig.ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "SetBIOSSetting signature unavailable",
                )
            })?;

            let mut in_params: Option<IWbemClassObject> = None;
            in_params = Some(in_sig.SpawnInstance(0)?);
            let in_params = in_params.ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "could not spawn in-params",
                )
            })?;

            put_string(&in_params, "Name", name);
            put_string(&in_params, "Password", "");
            put_string(&in_params, "Value", value);

            let mut out_params: Option<IWbemClassObject> = None;
            self.services.ExecMethod(
                &BSTR::from(self.instance_path.as_str()),
                &BSTR::from("SetBIOSSetting"),
                windows::Win32::System::Wmi::WBEM_FLAG_BIDIRECTIONAL,
                Some(&self.ctx),
                &in_params,
                Some(&mut out_params),
                None,
            )?;

            let out_params = out_params.ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "no out params from SetBIOSSetting",
                )
            })?;
            let code = get_u32(&out_params, "Return");
            Ok(SetResult::from_code(code))
        }
    }
}

// ---- VARIANT helpers (mirror the shapes proven in session.rs) ----

unsafe fn get_string(obj: &IWbemClassObject, prop: &str) -> String {
    let mut v = VARIANT::default();
    if obj.Get(&BSTR::from(prop), 0, &mut v, None, None).is_ok() {
        let s = v.to_string();
        let _ = VariantClear(&mut v);
        s
    } else {
        String::new()
    }
}

unsafe fn get_bool(obj: &IWbemClassObject, prop: &str) -> bool {
    let mut v = VARIANT::default();
    if obj.Get(&BSTR::from(prop), 0, &mut v, None, None).is_err() {
        return false;
    }
    let raw = v.as_raw().Anonymous.Anonymous;
    let out = raw.Anonymous.boolVal != 0;
    let _ = VariantClear(&mut v);
    out
}

/// `Return` is a CIM uint32, which WMI marshals as a signed 32-bit variant.
unsafe fn get_u32(obj: &IWbemClassObject, prop: &str) -> u32 {
    let mut v = VARIANT::default();
    if obj.Get(&BSTR::from(prop), 0, &mut v, None, None).is_err() {
        return u32::MAX;
    }
    let raw = v.as_raw().Anonymous.Anonymous;
    let vt = raw.vt;
    let out = if vt == 0 || vt == 1 {
        // VT_EMPTY / VT_NULL: treat as "no answer", not as success.
        u32::MAX
    } else if vt == 19 {
        raw.Anonymous.ulVal
    } else {
        raw.Anonymous.lVal as u32
    };
    let _ = VariantClear(&mut v);
    out
}

/// `PossibleValues` is a SAFEARRAY of BSTR. Returns empty if it is not that shape.
unsafe fn get_string_array(obj: &IWbemClassObject, prop: &str) -> Vec<String> {
    let mut v = VARIANT::default();
    if obj.Get(&BSTR::from(prop), 0, &mut v, None, None).is_err() {
        return Vec::new();
    }
    let raw = v.as_raw().Anonymous.Anonymous;
    // WMI returns string arrays as VT_ARRAY|VT_BSTR (0x2008) but some providers
    // hand back VT_ARRAY|VT_VARIANT (0x200C). Accept both; SafeArrayGetElement
    // fills a VARIANT either way.
    if raw.vt != 0x2008 && raw.vt != 0x200C {
        let _ = VariantClear(&mut v);
        return Vec::new();
    }
    let mut out = Vec::new();
    let psa: *mut windows::core::imp::SAFEARRAY = raw.Anonymous.parray;
    if !psa.is_null() {
        let sa_win: *const windows::Win32::System::Com::SAFEARRAY = psa as *const _;
        if let Ok(ub) = windows::Win32::System::Ole::SafeArrayGetUBound(sa_win, 1) {
            // VT_ARRAY|VT_BSTR stores raw BSTR pointers, so the destination must
            // be a BSTR* — reading into a VARIANT yields garbage. This was the
            // bug that made every 'possible' list print as empty strings.
            let lb = (*sa_win).rgsabound[0].lLbound;
            for i in lb..=ub {
                if raw.vt == 0x2008 {
                    let mut p: *mut u16 = std::ptr::null_mut();
                    if windows::Win32::System::Ole::SafeArrayGetElement(
                        sa_win,
                        &i,
                        &mut p as *mut _ as *mut core::ffi::c_void,
                    )
                    .is_ok()
                        && !p.is_null()
                    {
                        // Take ownership of the BSTR, convert, then drop it.
                        let b = BSTR::from_raw(p);
                        let s = b.to_string();
                        drop(b);
                        if !s.trim().is_empty() {
                            out.push(s);
                        }
                    }
                } else {
                    // VT_ARRAY|VT_VARIANT: each element is a real VARIANT.
                    let mut e = VARIANT::default();
                    if windows::Win32::System::Ole::SafeArrayGetElement(
                        sa_win,
                        &i,
                        &mut e as *mut _ as *mut core::ffi::c_void,
                    )
                    .is_ok()
                    {
                        let s = e.to_string();
                        let _ = VariantClear(&mut e);
                        if !s.trim().is_empty() {
                            out.push(s);
                        }
                    }
                }
            }
        }
    }
    let _ = VariantClear(&mut v);
    out
}

unsafe fn put_string(obj: &IWbemClassObject, prop: &str, val: &str) {
    let mut v = VARIANT::from(BSTR::from(val));
    let _ = obj.Put(&BSTR::from(prop), 0, &v, 0);
    let _ = VariantClear(&mut v);
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The calibrated return-code table. `Battery Care Function` returning 5 for
    /// every value is the evidence that the setting exists but is unusable here.
    #[test]
    fn return_code_table() {
        assert_eq!(SetResult::from_code(0), SetResult::Accepted);
        assert_eq!(SetResult::from_code(6), SetResult::AcceptedNeedsReboot);
        assert_eq!(SetResult::from_code(1), SetResult::UnknownSetting);
        assert_eq!(SetResult::from_code(5), SetResult::ValueRejected);
        assert_eq!(SetResult::from_code(9), SetResult::Other(9));
    }

    #[test]
    fn ok_only_for_accepted() {
        assert!(SetResult::Accepted.is_ok());
        assert!(SetResult::AcceptedNeedsReboot.is_ok());
        assert!(!SetResult::ValueRejected.is_ok());
        assert!(!SetResult::UnknownSetting.is_ok());
        assert!(!SetResult::Other(2).is_ok());
    }

    /// 1 and 5 must never collapse into one another: "no such setting" and
    /// "wrong value" are different diagnoses and drove opposite conclusions.
    #[test]
    fn unknown_and_rejected_are_distinct() {
        assert_ne!(
            SetResult::from_code(1),
            SetResult::from_code(5),
            "unknown-name and value-rejected must stay distinguishable"
        );
    }
}
