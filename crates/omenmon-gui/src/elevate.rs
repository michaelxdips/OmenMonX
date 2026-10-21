//! Elevation: detect, and relaunch elevated through the UAC prompt.
//!
//! The app is useful unelevated (battery, WMI temperatures), so it never refuses
//! to start. It offers the upgrade instead. Writes are the part that needs the
//! token, and every write path checks [`is_elevated`] before touching hardware so
//! the failure is a sentence rather than a raw `0x80041003`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// True if this process already has an elevated token.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Relaunch this executable elevated, forwarding the current arguments.
///
/// Returns `Ok(())` once the UAC prompt has been shown and accepted. The caller
/// should exit immediately afterwards: two processes sharing one WMI handle and
/// one pending-cap marker is exactly the race the marker cannot survive.
///
/// A refused UAC prompt is not an error the user needs a stack trace for, so the
/// message is returned as `Err` with plain text and the caller decides how loud
/// to be.
pub fn relaunch_elevated() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own exe: {e}"))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let params = args
        .iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // ShellExecuteW with the "runas" verb is the documented way to get the UAC
    // prompt. CreateProcess cannot: it would fail with ERROR_ELEVATION_REQUIRED
    // instead of asking.
    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params_w = wide(&params);

    let result = unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params_w.as_ptr()),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW returns a fake HINSTANCE; values <= 32 are error codes.
    let code = result.0 as usize;
    if code <= 32 {
        // 1223 = ERROR_CANCELLED, the user pressed No on the UAC dialog.
        let why = match code {
            5 => "access denied".to_string(),
            1223 => "cancelled at the UAC prompt".to_string(),
            other => format!("ShellExecuteW failed (code {other})"),
        };
        return Err(why);
    }
    Ok(())
}

/// Copy text to the Windows clipboard.
///
/// Implemented directly rather than through a crate: the app already links
/// user32, and a clipboard dependency for one button is not worth the surface.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        OpenClipboard(None).map_err(|e| format!("clipboard busy: {e}"))?;
        // Ensure the clipboard is closed on every early return below.
        let result = (|| -> Result<(), String> {
            EmptyClipboard().map_err(|e| format!("empty failed: {e}"))?;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| format!("alloc: {e}"))?;
            let p = GlobalLock(h);
            if p.is_null() {
                return Err("lock failed".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p as *mut u16, wide.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(CF_UNICODETEXT, HANDLE(h.0 as _))
                .map_err(|e| format!("set failed: {e}"))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Suppress the console window a child process would otherwise flash.
pub fn hidden_command(program: impl AsRef<OsStr>) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = std::process::Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wide-string helper must NUL-terminate exactly once; every Win32 call
    /// above depends on it.
    #[test]
    fn wide_is_nul_terminated() {
        let w = wide("omenmon");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(w.len(), "omenmon".len() + 1);
        assert_eq!(w[0], b'o' as u16);
    }

    /// Argument quoting: a path with a space must survive the round trip through
    /// ShellExecuteW, or "C:\Program Files\OmenMon\omenmon-gui.exe" launches the
    /// wrong thing.
    #[test]
    fn args_with_spaces_are_quoted() {
        let args = vec!["--install-dir".to_string(), r"C:\Program Files\OmenMon".to_string()];
        let params = args
            .iter()
            .map(|a| {
                if a.contains(' ') || a.contains('"') {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(params, "--install-dir \"C:\\Program Files\\OmenMon\"");
    }
}
