//! Per-user OLE class registration for the embedded-object server
//! (genoffice#1985 stage 3). `--register` writes the CLSID under
//! HKCU\Software\Classes (merged into HKCR, no elevation needed) so that
//! double-clicking a pasted GenOffice object makes ole32 launch
//! `win-clipboard-helper.exe --ole-server`, which answers the activation and
//! opens the workbook in GenOffice. `--unregister` removes the keys.
//!
//! Key layout follows the minimal LocalServer32 shape:
//!   CLSID\{guid}\              (default) = user type name
//!   CLSID\{guid}\LocalServer32 (default) = "<helper>" --ole-server

use crate::embed;

/// The CLSID subtree this helper owns, relative to HKCU.
pub fn clsid_registry_path() -> String {
    format!(
        "Software\\Classes\\CLSID\\{{{}}}",
        embed::GENOFFICE_SHEET_CLSID.to_string().to_uppercase()
    )
}

/// The LocalServer32 command: the quoted helper path plus the server flag.
pub fn local_server_command(helper_path: &str) -> String {
    format!("\"{}\" --ole-server", helper_path)
}

/// The user type name stored as the CLSID key's default value.
pub const USER_TYPE_NAME: &str = "GenOffice Worksheet";

#[cfg(windows)]
pub fn register(helper_path: &str) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn set_default_value(key: HKEY, data: &[u16]) -> Result<(), String> {
        let status = unsafe {
            RegSetValueExW(
                key,
                std::ptr::null(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("RegSetValueExW failed: {status:#x}"))
        }
    }

    unsafe fn create_subkey(parent: HKEY, subkey: &[u16]) -> Result<HKEY, String> {
        let mut key: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                parent,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status == 0 {
            Ok(key)
        } else {
            Err(format!("RegCreateKeyExW failed: {status:#x}"))
        }
    }

    let class_path = wide(&clsid_registry_path());
    let server_path = wide("LocalServer32");
    unsafe {
        let class_key = create_subkey(HKEY_CURRENT_USER, &class_path)?;
        let result = (|| {
            set_default_value(class_key, &wide(USER_TYPE_NAME))?;
            let server_key = create_subkey(class_key, &server_path)?;
            let result = set_default_value(server_key, &wide(&local_server_command(helper_path)));
            RegCloseKey(server_key);
            result
        })();
        RegCloseKey(class_key);
        result
    }
}

#[cfg(windows)]
pub fn unregister() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};

    let path = clsid_registry_path();
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr()) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("RegDeleteTreeW failed: {status:#x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_path_names_the_clsid_subtree() {
        let path = clsid_registry_path();
        assert!(path.starts_with("Software\\Classes\\CLSID\\{"));
        assert!(path.ends_with('}'));
        assert_eq!(
            path,
            format!(
                "Software\\Classes\\CLSID\\{{{}}}",
                embed::GENOFFICE_SHEET_CLSID.to_string().to_uppercase()
            )
        );
    }

    #[test]
    fn command_quotes_the_helper_and_names_the_server_flag() {
        let command = local_server_command("C:\\app\\resources\\native\\win-clipboard-helper.exe");
        assert_eq!(
            command,
            "\"C:\\app\\resources\\native\\win-clipboard-helper.exe\" --ole-server"
        );
    }
}
