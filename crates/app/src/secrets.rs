//! Secrets in the system keychain: tokens of add-ons and git logins. The
//! keychain keeps them encrypted, and they never touch a file of the editor.

use std::{
    io::Write,
    process::{Command, Stdio},
};

/// Reads a secret. Blocks.
pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security").args(["find-generic-password", "-s", service, "-a", account, "-w"]).output()
    } else {
        Command::new("secret-tool").args(["lookup", "service", service, "account", account]).output()
    }
    .map_err(|error| format!("The system keychain is unavailable: {error}"))?;
    let value = String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string();
    Ok((output.status.success() && !value.is_empty()).then_some(value))
}

/// Stores a secret. The value goes through stdin, because arguments show in `ps`. Blocks.
pub fn set(service: &str, label: &str, account: &str, value: &str) -> Result<(), String> {
    let (mut command, input) = if cfg!(target_os = "macos") {
        let mut command = Command::new("security");
        command.arg("-i");
        let line = format!("add-generic-password -U -s {} -a {} -w {}\n", quote(service), quote(account), quote(value));
        (command, line)
    } else {
        let mut command = Command::new("secret-tool");
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

/// Quotes one word for `security -i`.
fn quote(word: &str) -> String {
    format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_words_are_quoted() {
        assert_eq!(quote(r#"a"b\c d"#), r#""a\"b\\c d""#);
    }
}
