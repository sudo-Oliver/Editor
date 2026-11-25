//! Secrets in the system keychain: tokens of add-ons and git logins. The
//! keychain keeps them encrypted, and they never touch a file of the editor.
//! On Windows that is the Credential Manager, under `service/account`.

#[cfg(not(windows))]
use std::{io::Write, process::Stdio};

/// Reads a secret. Blocks.
#[cfg(windows)]
pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
    credentials::read(&format!("{service}/{account}"))
}

/// Stores a secret. Blocks.
#[cfg(windows)]
pub fn set(service: &str, _label: &str, account: &str, value: &str) -> Result<(), String> {
    credentials::write(&format!("{service}/{account}"), account, value)
}

/// Reads a secret. Blocks.
#[cfg(not(windows))]
pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
    let output = if cfg!(target_os = "macos") {
        crate::platform::command("security").args(["find-generic-password", "-s", service, "-a", account, "-w"]).output()
    } else {
        crate::platform::command("secret-tool").args(["lookup", "service", service, "account", account]).output()
    }
    .map_err(|error| format!("The system keychain is unavailable: {error}"))?;
    let value = String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string();
    Ok((output.status.success() && !value.is_empty()).then_some(value))
}

/// Stores a secret. The value goes through stdin, because arguments show in `ps`. Blocks.
#[cfg(not(windows))]
pub fn set(service: &str, label: &str, account: &str, value: &str) -> Result<(), String> {
    let (mut command, input) = if cfg!(target_os = "macos") {
        let mut command = crate::platform::command("security");
        command.arg("-i");
        let line = format!("add-generic-password -U -s {} -a {} -w {}\n", quote(service), quote(account), quote(value));
        (command, line)
    } else {
        let mut command = crate::platform::command("secret-tool");
        command.args(["store", "--label", label, "service", service, "account", account]);
        (command, value.to_string())
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("The system keychain is unavailable: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|error| format!("Cannot store the secret: {error}"))?;
    }
    let output = child.wait_with_output().map_err(|error| format!("Cannot store the secret: {error}"))?;
    let failure = String::from_utf8_lossy(&output.stderr);
    if output.status.success() && failure.trim().is_empty() {
        Ok(())
    } else {
        // Not the input, which holds the secret.
        Err("The system keychain refused the secret.".to_string())
    }
}

#[cfg(windows)]
mod credentials {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};

    use windows_sys::Win32::{
        Foundation::{ERROR_NOT_FOUND, FILETIME, GetLastError},
        Security::Credentials::{CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW, CredWriteW},
    };

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    pub fn read(target: &str) -> Result<Option<String>, String> {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: a nul-terminated name and an out pointer that CredFree releases.
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                return if GetLastError() == ERROR_NOT_FOUND { Ok(None) } else { Err("The Credential Manager is unavailable.".to_string()) };
            }
            let blob = std::slice::from_raw_parts((*credential).CredentialBlob, (*credential).CredentialBlobSize as usize);
            let value = String::from_utf8_lossy(blob).into_owned();
            CredFree(credential.cast());
            Ok(Some(value).filter(|value| !value.is_empty()))
        }
    }

    pub fn write(target: &str, account: &str, value: &str) -> Result<(), String> {
        let (mut target, mut account) = (wide(target), wide(account));
        let mut blob = value.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: account.as_mut_ptr(),
        };
        // SAFETY: every pointer lives until the call returns.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err("The Credential Manager refused the secret.".to_string());
        }
        Ok(())
    }
}

/// Quotes one word for `security -i`.
#[cfg_attr(windows, allow(dead_code))]
fn quote(word: &str) -> String {
    format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A login stays in the Credential Manager, as a git token does between starts.
    #[cfg(windows)]
    #[test]
    fn the_credential_manager_keeps_a_secret() {
        let service = format!("editor-test-{}", std::process::id());
        assert_eq!(get(&service, "ada"), Ok(None));
        set(&service, "", "ada", "token-ä1").unwrap();
        assert_eq!(get(&service, "ada"), Ok(Some("token-ä1".to_string())));
        set(&service, "", "ada", "token-2").unwrap();
        assert_eq!(get(&service, "ada"), Ok(Some("token-2".to_string())));
        std::process::Command::new("cmdkey").arg(format!("/delete:{service}/ada")).output().ok();
        assert_eq!(get(&service, "ada"), Ok(None));
    }

    #[test]
    fn keychain_words_are_quoted() {
        assert_eq!(quote(r#"a"b\c d"#), r#""a\"b\\c d""#);
    }
}
