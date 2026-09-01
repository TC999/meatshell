//! Windows Explorer context menu: right-click a folder → "在此处打开
//! Meatshell", launching `meatshell --new-window --dir <folder>`.
//! Explorer passes the currently selected item via the `%V` verb token, so on
//! folder selection the argument is a directory path.  The launch is
//! forwarded to the running primary over the single-instance IPC socket
//! (see `single_instance.rs`), so the entry behaves like the taskbar
//! "新建窗口" task instead of spawning a second process.
//!
//! Registration runs at startup and must never block or fail startup: every
//! error path degrades to a tracing warn.  We call the Win32 registry
//! functions directly (via FFI to `advapi32.dll`) rather than pulling in
//! another registry crate — windows 0.58 doesn't expose
//! `Win32_System_WinReg` and we want the lock diff to stay minimal.

use std::path::PathBuf;

/// HKCR raw handle value (matches `HKEY_CLASSES_ROOT` from WinReg).
const HKEY_CLASSES_ROOT: isize = 0x80000000u32 as isize;
const REG_SZ: u32 = 1;
const REG_OPTION_NON_VOLATILE: u32 = 0x0000_0001;
const KEY_WRITE: u32 = 0x20006;
const ERROR_SUCCESS: u32 = 0;

/// Explorer verb key paths — Explorer dispatches shell verbs under
/// `Directory\shell\<verb>` (folder) and `Directory\Background\shell\<verb>`
/// (the empty Explorer canvas) so the entry appears on both.  We also
/// register under `*\shell\<verb>` so a file right-click offers the same
/// entry (Explorer resolves `%V` to the file path, and the cwd the shell
/// opens in is the file's parent directory).
const VERB_KEYS: &[&str] = &[
    "Directory\\shell\\meatshell",
    "Directory\\Background\\shell\\meatshell",
    "*\\shell\\meatshell",
];
const COMMAND_SUBKEY: &str = "command";

const VERB_NAME: &str = "在此处打开 Meatshell";

/// Build `"<exe>" --new-window --dir "%V"` for Explorer's command line.
fn exe_arg() -> String {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("meatshell.exe"));
    format!("\"{}\" --new-window --dir \"%V\"", exe.to_string_lossy())
}

/// Install the Explorer context-menu verb.  Failures are logged and
/// swallowed — a missing context-menu entry must never keep the app from
/// starting.
pub fn register_directory_menu() {
    if let Err(e) = register_inner() {
        tracing::warn!("directory context menu registration failed: {e}");
    }
}

fn register_inner() -> std::io::Result<()> {
    let cmd = exe_arg();

    // Remove any stale entry from a previous install path so a poisoned
    // command line is never left behind.
    for key in VERB_KEYS {
        let _ = unregister_key(key);
    }

    for key in VERB_KEYS {
        let parts = split_key_path(key)?;
        let mut hverb: isize = 0;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CLASSES_ROOT,
                parts.sub.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hverb as *mut isize,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        set_reg_sz(hverb, None, VERB_NAME)?;
        let hcmd_wide = os_str_to_wide(COMMAND_SUBKEY);
        let mut hcmd: isize = 0;
        let status = unsafe {
            RegCreateKeyExW(
                hverb,
                hcmd_wide.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hcmd as *mut isize,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        set_reg_sz(hcmd, None, &cmd)?;
        // Close both keys.  CloseHandle returns non-zero on success.
        let _ = unsafe { CloseHandle(hverb) };
        let _ = unsafe { CloseHandle(hcmd) };
    }
    tracing::info!(key = %VERB_KEYS.join("; "), "directory context menu registered");
    Ok(())
}

/// Best-effort removal of the context-menu verb.  Silently tolerates a
/// missing key; used by a future uninstaller / settings toggle.
pub fn unregister_directory_menu() {
    if let Err(e) = unregister_inner() {
        tracing::warn!("directory context menu unregistration failed: {e}");
    }
}

fn unregister_inner() -> std::io::Result<()> {
    for key in VERB_KEYS {
        let _ = unregister_key(key);
    }
    Ok(())
}

fn unregister_key(key: &str) -> std::io::Result<()> {
    let parts = split_key_path(key)?;
    let status = unsafe { RegDeleteTreeW(HKEY_CLASSES_ROOT, parts.sub.as_ptr()) };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

fn set_reg_sz(hkey: isize, name: Option<&str>, value: &str) -> std::io::Result<()> {
    let value_wide = os_str_to_wide(value);
    let cb = (value_wide.len() * 2) as u32;
    let name_wide = name.map(os_str_to_wide).unwrap_or_default();
    let name_ptr = if name_wide.is_empty() {
        std::ptr::null()
    } else {
        name_wide.as_ptr()
    };
    let data_ptr: *const u8 = value_wide.as_ptr() as *const u8;
    let status = unsafe {
        RegSetValueExW(hkey, name_ptr, 0, REG_SZ, data_ptr, cb)
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

struct KeyParts {
    sub: Vec<u16>,
}

fn split_key_path(path: &str) -> std::io::Result<KeyParts> {
    path.split("\\").nth(1).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "malformed registry key path")
    })?;
    let full_sub: Vec<u16> = path
        .split("\\")
        .skip(1)
        .collect::<Vec<&str>>()
        .join("\\")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    Ok(KeyParts { sub: full_sub })
}

fn os_str_to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        key: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        security_attributes: *const u8,
        phk_result: *mut isize,
        lpdw_disposition: *mut u32,
    ) -> u32;
    fn RegDeleteTreeW(key: isize, sub_key: *const u16) -> u32;
    fn RegSetValueExW(
        key: isize,
        value_name: *const u16,
        reserved: u32,
        r#type: u32,
        data: *const u8,
        data_size: u32,
    ) -> u32;
    fn CloseHandle(handle: isize) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_context_menu_command_with_dir_flag() {
        let cmd = exe_arg();
        assert!(cmd.contains("--new-window"));
        assert!(cmd.contains("--dir"));
        assert!(cmd.contains("%V"));
        assert!(cmd.starts_with('"'));
    }

    #[test]
    fn wide_conversion_terminates() {
        let w = os_str_to_wide("test");
        assert_eq!(*w.last().unwrap(), 0);
    }

    #[test]
    fn split_key_path_returns_full_subkey() {
        let parts = split_key_path("Directory\\shell\\meatshell").unwrap();
        let sub_str = String::from_utf16_lossy(&parts.sub);
        assert_eq!(sub_str, "shell\\meatshell");
    }

    #[test]
    fn split_bg_key_path_returns_full_subkey() {
        let parts = split_key_path("Directory\\Background\\shell\\meatshell").unwrap();
        let sub_str = String::from_utf16_lossy(&parts.sub);
        assert_eq!(sub_str, "Background\\shell\\meatshell");
    }
}
