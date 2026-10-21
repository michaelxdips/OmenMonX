//! Session: raw COM/WMI transport to hpqBIntM, replicating the exact invocation
//! pattern validated live on Victus 8C9C (2026-10):
//!   - hpqBDataIn built manually with InstanceName (key) + Active properties
//!   - InData passed as embedded instance object
//!   - Size = 0 when payload empty
//!   - OutData is an embedded object with rwReturnCode + OutData byte array

use windows::core::{imp, BSTR, IUnknown, Interface, VARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoInitializeSecurity, CoSetProxyBlanket, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, SAFEARRAY,
};
use windows::Win32::System::Wmi::IWbemLocator;
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayCreateVector, SafeArrayGetElement, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};

use windows::Win32::System::Variant::{VT_ARRAY, VT_UI1};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemContext, IWbemServices, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemContext,
};

use crate::error::{Error, Result};
use crate::payload::{Request, Response};

const NAMESPACE: &str = r"root\wmi";
const METHOD_CLASS: &str = "hpqBIntM";
const METHOD_INSTANCE: &str = r"ACPI\PNP0C14\0_0";
const DATA_IN_CLASS: &str = "hpqBDataIn";

/// Out-method name per buffer size, matching OmenMon's BIOS_METHOD table.
pub fn method_for_size(out_size: usize) -> &'static str {
    match out_size {
        0 => "hpqBIOSInt0",
        4 => "hpqBIOSInt4",
        128 => "hpqBIOSInt128",
        1024 => "hpqBIOSInt1024",
        4096 => "hpqBIOSInt4096",
        _ => "hpqBIOSInt128",
    }
}

pub struct HpWmi {
    services: IWbemServices,
    ctx: IWbemContext,
    instance_path: String,
    data_in_class: IWbemClassObject,
    method_in: std::collections::HashMap<&'static str, IWbemClassObject>,
}

unsafe impl Send for HpWmi {}
unsafe impl Sync for HpWmi {}

impl HpWmi {
    pub fn connect() -> Result<Self> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() && hr.0 != 1 {
                // S_FALSE means already initialized â€” fine.
                return Err(Error::ComInit(hr.ok().unwrap_err()));
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

            // CLSID_WbemLocator = {4590F811-1D3A-11D0-891F-00AA004B2E24}
            let wbem_locator_clsid = windows::core::GUID::from_u128(0x4590F811_1D3A_11D0_891F_00AA004B2E24);
            let locator: IWbemLocator =
                CoCreateInstance(&wbem_locator_clsid, None, CLSCTX_INPROC_SERVER).map_err(Error::Windows)?;

            let services = locator
                .ConnectServer(
                    &BSTR::from(NAMESPACE),
                    &BSTR::new(),
                    &BSTR::new(),
                    &BSTR::new(),
                    0,
                    &BSTR::new(),
                    None,
                )
                .map_err(|source| Error::Connect { namespace: NAMESPACE.into(), source })?;

            CoSetProxyBlanket(
                &services,
                windows::Win32::System::Rpc::RPC_C_AUTHN_WINNT,
                windows::Win32::System::Rpc::RPC_C_AUTHZ_NONE,
                None,
                windows::Win32::System::Com::RPC_C_AUTHN_LEVEL_CALL,
                windows::Win32::System::Com::RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                windows::Win32::System::Com::EOAC_NONE,
            )
            .map_err(Error::Windows)?;

            // Verify the method instance exists (enumerate all — WQL backslash
            // escaping is error-prone; filter client-side like the validated probe).
            let query = BSTR::from(format!("SELECT * FROM {METHOD_CLASS}"));
            let ctx: IWbemContext = CoCreateInstance(&WbemContext, None, CLSCTX_INPROC_SERVER).map_err(Error::Windows)?;
            let enumerator = services
                .ExecQuery(&BSTR::from("WQL"), &query, WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, Some(&ctx))
                .map_err(Error::Windows)?;

            let mut found = false;
            let mut instance_path_captured = String::new();
            loop {
                let mut objs = [None; 1];
                let mut returned = 0;
                enumerator.Next(WBEM_INFINITE, &mut objs, &mut returned).ok().map_err(Error::Windows)?;
                if returned == 0 {
                    break;
                }
                if let Some(obj) = &objs[0] {
                    found = true;
                    let mut pv = VARIANT::default();
                    if obj.Get(&BSTR::from("__PATH"), 0, &mut pv, None, None).is_ok() {
                        instance_path_captured = pv.to_string();
                    }
                }
            }
            if !found {
                return Err(Error::NoInstance);
            }

            let mut data_in_class: Option<IWbemClassObject> = None;
            services
                .GetObject(&BSTR::from(DATA_IN_CLASS), windows::Win32::System::Wmi::WBEM_FLAG_RETURN_WBEM_COMPLETE, Some(&ctx), Some(&mut data_in_class), None)
                .map_err(Error::Windows)?;
            let data_in_class = data_in_class.ok_or(Error::Shape("hpqBDataIn class not found"))?;

            let mut method_class: Option<IWbemClassObject> = None;
            services
                .GetObject(&BSTR::from(METHOD_CLASS), windows::Win32::System::Wmi::WBEM_FLAG_RETURN_WBEM_COMPLETE, Some(&ctx), Some(&mut method_class), None)
                .map_err(Error::Windows)?;
            let method_class = method_class.ok_or(Error::Shape("hpqBIntM class not found"))?;

            let mut method_in = std::collections::HashMap::new();
            for name in ["hpqBIOSInt0", "hpqBIOSInt4", "hpqBIOSInt128", "hpqBIOSInt1024", "hpqBIOSInt4096"] {
                let mut in_sig: Option<IWbemClassObject> = None;
                let mut out_sig: Option<IWbemClassObject> = None;
                method_class
                    .GetMethod(&windows::core::HSTRING::from(name), 0, &mut in_sig, &mut out_sig)
                    .map_err(|source| Error::Invoke { method: name.into(), source })?;
                if let Some(sig) = in_sig {
                    method_in.insert(name, sig);
                }
            }

            Ok(Self { services, ctx, instance_path: instance_path_captured, data_in_class, method_in })
        }
    }

    pub fn send(&self, req: &Request, out_size: usize) -> Result<Response> {
        let method = method_for_size(out_size);
        unsafe {
            let input = self.data_in_class.SpawnInstance(0).map_err(Error::Windows)?;

            put_byte_array(&input, "Sign", &crate::payload::SIGN)?;
            put_u32(&input, "Command", req.command)?;
            put_u32(&input, "CommandType", req.command_type)?;
            put_u32(&input, "Size", req.size())?;
            if !req.data.is_empty() {
                put_byte_array(&input, "hpqBData", &req.data)?;
            }

            let sig = self.method_in.get(method).ok_or(Error::Shape("method signature not cached"))?;
            let in_params = sig.SpawnInstance(0).map_err(Error::Windows)?;
            put_variant(&in_params, "InData", variant_from_object(&input)?)?;

            // Use the FULL __PATH captured from enumeration (includes server name —
            // some WDM providers require the fully-qualified object path).
            let instance_path = BSTR::from(self.instance_path.clone());
            
            let mut out_params: Option<IWbemClassObject> = None;
            self.services
                .ExecMethod(&instance_path, &BSTR::from(method), windows::Win32::System::Wmi::WBEM_FLAG_BIDIRECTIONAL, Some(&self.ctx), &in_params, Some(&mut out_params), None)
                .map_err(|source| Error::Invoke { method: method.into(), source })?;
            let out_params = out_params.ok_or(Error::Shape("ExecMethod returned no out params"))?;

            // ReturnValue may be empty/void on this provider — treat missing as success
            // and rely on rwReturnCode from the embedded OutData instance.
            if let Ok(ret) = get_bool(&out_params, "ReturnValue") {
                if !ret {
                    return Err(Error::ReturnedFalse { method: method.into() });
                }
            }

            let out_obj = get_object(&out_params, "OutData")?;
            let rw = get_u32(&out_obj, "rwReturnCode")?;
            // Victus hpqBDataOut* classes name the payload property "Data"
            // (Omen boards used "OutData") — try both.
            let data = match get_byte_array(&out_obj, "Data") {
                Ok(v) if !v.is_empty() => v,
                _ => get_byte_array(&out_obj, "OutData")
                    .unwrap_or_default(),
            };

            Ok(Response { return_code: rw, data })
        }
    }
}

// ---- VARIANT helpers (raw construction mirrors windows-core From impls) ----

fn variant_from_object(obj: &IWbemClassObject) -> Result<VARIANT> {
    let unk: IUnknown = Interface::cast::<IUnknown>(obj).map_err(|_| Error::Shape("to IUnknown failed"))?;
    Ok(VARIANT::from(unk))
}

unsafe fn variant_from_byte_array(data: &[u8]) -> Result<VARIANT> {
    let sa = SafeArrayCreateVector(VT_UI1, 0, data.len() as u32);
    if sa.is_null() {
        return Err(Error::Shape("SafeArrayCreateVector failed"));
    }
    if !data.is_empty() {
        let mut ptr = std::ptr::null_mut();
        SafeArrayAccessData(sa, &mut ptr).map_err(Error::Windows)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        SafeArrayUnaccessData(sa).map_err(Error::Windows)?;
    }
    let raw = imp::VARIANT {
        Anonymous: imp::VARIANT_0 {
            Anonymous: imp::VARIANT_0_0 {
                vt: 8192u16 | 17u16,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: imp::VARIANT_0_0_0 { parray: sa as *mut windows::core::imp::SAFEARRAY },
            },
        },
    };
    Ok(std::mem::transmute::<imp::VARIANT, VARIANT>(raw))
}

unsafe fn put_variant(obj: &IWbemClassObject, name: &str, mut v: VARIANT) -> Result<()> {
    obj.Put(&BSTR::from(name), 0, &mut v, 0).map_err(Error::Windows)
}

unsafe fn put_u32(obj: &IWbemClassObject, name: &str, val: u32) -> Result<()> {
    // CIM uint32 is represented as VT_I4 in WMI (CIM_UINT32 maps to signed i32 ABI).
    put_variant(obj, name, VARIANT::from(val as i32))
}

unsafe fn put_byte_array(obj: &IWbemClassObject, name: &str, data: &[u8]) -> Result<()> {
    put_variant(obj, name, variant_from_byte_array(data)?)
}

unsafe fn get_bool(obj: &IWbemClassObject, name: &str) -> Result<bool> {
    let mut v = VARIANT::default();
    obj.Get(&BSTR::from(name), 0, &mut v, None, None).map_err(Error::Windows)?;
    let raw = v.as_raw().Anonymous.Anonymous;
    Ok(raw.Anonymous.boolVal != 0)
}

unsafe fn get_u32(obj: &IWbemClassObject, name: &str) -> Result<u32> {
    let mut v = VARIANT::default();
    obj.Get(&BSTR::from(name), 0, &mut v, None, None).map_err(Error::Windows)?;
    let raw = v.as_raw().Anonymous.Anonymous;
    if raw.vt == imp::VT_EMPTY || raw.vt == 1u16 {
        return Ok(0);
    }
    if raw.vt == 19u16 {
        Ok(raw.Anonymous.ulVal)
    } else {
        Ok(raw.Anonymous.lVal as u32)
    }
}

unsafe fn get_object(obj: &IWbemClassObject, name: &str) -> Result<IWbemClassObject> {
    let mut v = VARIANT::default();
    obj.Get(&BSTR::from(name), 0, &mut v, None, None).map_err(Error::Windows)?;
    let raw = v.as_raw().Anonymous.Anonymous;
    if raw.vt == imp::VT_EMPTY || raw.vt == 1u16 || raw.Anonymous.punkVal.is_null() {
        return Err(Error::MissingOutParam { name: name.into() });
    }
    let unk = IUnknown::from_raw_borrowed(&raw.Anonymous.punkVal).unwrap().clone();
    unk.cast::<IWbemClassObject>()
        .map_err(|_| Error::Shape("OutData is not an embedded object"))
}

unsafe fn get_byte_array(obj: &IWbemClassObject, name: &str) -> Result<Vec<u8>> {
    let mut v = VARIANT::default();
    obj.Get(&BSTR::from(name), 0, &mut v, None, None).map_err(Error::Windows)?;
    let raw = v.as_raw().Anonymous.Anonymous;
    if raw.vt == imp::VT_EMPTY || raw.vt == 1u16 {
        return Ok(Vec::new());
    }
    let sa: *mut windows::core::imp::SAFEARRAY = raw.Anonymous.parray;
    if sa.is_null() {
        return Ok(Vec::new());
    }
    let sa_win: *const SAFEARRAY = std::mem::transmute(sa);
    let ub = SafeArrayGetUBound(sa_win, 1).map_err(Error::Windows)?;
    let n = (ub + 1) as usize;
    if n == 0 {
        return Ok(Vec::new());
    }
    // Fast path: direct bulk read via SafeArrayAccessData (GetElement is unreliable here).
    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    SafeArrayAccessData(sa_win, &mut ptr).map_err(Error::Windows)?;
    let mut out = vec![0u8; n];
    std::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), n);
    SafeArrayUnaccessData(sa_win).map_err(Error::Windows)?;
    Ok(out)
}

























unsafe fn put_str(obj: &IWbemClassObject, name: &str, val: &str) -> Result<()> {
    put_variant(obj, name, VARIANT::from(val))
}

unsafe fn put_bool(obj: &IWbemClassObject, name: &str, val: bool) -> Result<()> {
    put_variant(obj, name, VARIANT::from(val))
}












