//! Signing in to GitHub without a terminal or a token to copy: the editor
//! shows a short code, the browser opens GitHub, and the user confirms there
//! (GitHub's device flow). The token then lives in the keychain like a pasted
//! one, and git gets it through the editor's askpass.

use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::Deserialize;

/// The OAuth app of the editor, with the device flow turned on. Public by
/// design, not a secret. Empty until the app is registered, which hides the
/// browser sign-in and leaves the token field.
pub const CLIENT_ID: &str = "";
pub const HOST: &str = "github.com";
const SECONDS: &str = "30";

pub fn available() -> bool {
    !CLIENT_ID.is_empty()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// What the user types at GitHub, such as `WDJB-MJHT`.
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
    pub id: u64,
    pub name: Option<String>,
}

impl User {
    /// The address GitHub keeps private for commits, so that no real address
    /// becomes public.
    pub fn noreply_email(&self) -> String {
        format!("{}+{}@users.noreply.github.com", self.id, self.login)
    }
}

#[derive(Debug, Deserialize)]
struct CreatedRepository {
    clone_url: String,
}

/// Talks to GitHub. `header` goes through stdin, so that no token shows up
/// in the list of processes.
fn request(method: &str, url: &str, header: Option<&str>, form: &[(&str, &str)], json: Option<&str>) -> Result<(u16, String), String> {
    let mut command = Command::new("curl");
    command.args(["--silent", "--show-error", "--location", "--max-time", SECONDS, "--proto", "=https", "--proto-redir", "=https"]);
    command.args(["--request", method, "--header", "Accept: application/json", "--write-out", "\n%{http_code}"]);
    if header.is_some() {
        command.args(["--header", "@-"]);
    }
    for (key, value) in form {
        command.arg("--data-urlencode").arg(format!("{key}={value}"));
    }
    if let Some(json) = json {
        command.args(["--header", "Content-Type: application/json", "--data", json]);
    }
    let mut child = command
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run curl: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(header) = header {
            stdin.write_all(header.as_bytes()).map_err(|error| format!("Cannot reach GitHub: {error}"))?;
        }
    }
    let output = child.wait_with_output().map_err(|error| format!("Cannot reach GitHub: {error}"))?;
    if !output.status.success() {
        return Err(format!("Cannot reach GitHub. Check the internet connection. ({})", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((status.trim().parse().unwrap_or(0), body.to_string()))
}

fn bearer(token: &str) -> String {
    format!("Authorization: Bearer {token}")
}

/// Asks GitHub for a code to show. Blocks.
pub fn request_code() -> Result<DeviceCode, String> {
    let (_, body) = request("POST", "https://github.com/login/device/code", None, &[("client_id", CLIENT_ID), ("scope", "repo")], None)?;
    serde_json::from_str(&body).map_err(|_| "GitHub did not give a code. Try again in a moment.".to_string())
}

/// What one look at the sign-in says.
#[derive(Debug, PartialEq, Eq)]
enum Poll {
    Token(String),
    Wait,
    /// GitHub wants fewer questions.
    SlowDown(u64),
    Failed(String),
}

fn read_poll(body: &str) -> Poll {
    let Ok(answer) = serde_json::from_str::<TokenAnswer>(body) else {
        return Poll::Failed("GitHub gave an answer the editor does not understand.".to_string());
    };
    if let Some(token) = answer.access_token.filter(|token| !token.is_empty()) {
        return Poll::Token(token);
    }
    match answer.error.as_deref() {
        Some("authorization_pending") => Poll::Wait,
        Some("slow_down") => Poll::SlowDown(answer.interval.unwrap_or(10)),
        Some("expired_token") => Poll::Failed("The code expired. Start the sign-in again.".to_string()),
        Some("access_denied") => Poll::Failed("The sign-in was declined at GitHub.".to_string()),
        Some(other) => Poll::Failed(format!("GitHub refused the sign-in: {other}")),
        None => Poll::Failed("GitHub gave no token.".to_string()),
    }
}

/// Waits until the user confirmed the code at GitHub, and returns the
/// token. Blocks.
pub fn wait_for_token(code: &DeviceCode, cancel: &AtomicBool) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = code.interval.max(5);
    loop {
        let slept = Instant::now();
        while slept.elapsed() < Duration::from_secs(interval) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Stopped.".to_string());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if Instant::now() > deadline {
            return Err("The code expired. Start the sign-in again.".to_string());
        }
        let form = [("client_id", CLIENT_ID), ("device_code", code.device_code.as_str()), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")];
        let (_, body) = request("POST", "https://github.com/login/oauth/access_token", None, &form, None)?;
        match read_poll(&body) {
            Poll::Token(token) => return Ok(token),
            Poll::Wait => {}
            Poll::SlowDown(next) => interval = next.max(interval + 5),
            Poll::Failed(error) => return Err(error),
        }
    }
}

/// Who the token belongs to. Blocks.
pub fn user(token: &str) -> Result<User, String> {
    let (status, body) = request("GET", "https://api.github.com/user", Some(&bearer(token)), &[], None)?;
    if status != 200 {
        return Err("GitHub did not accept the login.".to_string());
    }
    serde_json::from_str(&body).map_err(|_| "GitHub gave an answer the editor does not understand.".to_string())
}

/// Makes a new repository in the user's account and returns its URL for git. Blocks.
pub fn create_repository(token: &str, name: &str, private: bool) -> Result<String, String> {
    let json = serde_json::json!({ "name": name, "private": private }).to_string();
    let (status, body) = request("POST", "https://api.github.com/user/repos", Some(&bearer(token)), &[], Some(&json))?;
    match status {
        201 => serde_json::from_str::<CreatedRepository>(&body).map(|created| created.clone_url).map_err(|_| "GitHub gave an answer the editor does not understand.".to_string()),
        422 => Err(format!("You already have a repository named {name} on GitHub.")),
        401 | 403 => Err("GitHub did not allow it. Sign in again.".to_string()),
        _ => Err(format!("GitHub could not create the repository (status {status}).")),
    }
}

/// A repository name GitHub takes, made from a folder name.
pub fn repository_name(folder: &str) -> String {
    let name: String = folder.chars().map(|character| if character.is_ascii_alphanumeric() || "-_.".contains(character) { character } else { '-' }).collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() { "document".to_string() } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answers_of_the_device_flow_are_read() {
        assert_eq!(read_poll(r#"{"access_token":"gho_1","token_type":"bearer"}"#), Poll::Token("gho_1".to_string()));
        assert_eq!(read_poll(r#"{"error":"authorization_pending"}"#), Poll::Wait);
        assert_eq!(read_poll(r#"{"error":"slow_down","interval":10}"#), Poll::SlowDown(10));
        assert!(matches!(read_poll(r#"{"error":"expired_token"}"#), Poll::Failed(error) if error.contains("expired")));
        assert!(matches!(read_poll("<html>"), Poll::Failed(_)));
    }

    #[test]
    fn folder_names_become_repository_names() {
        assert_eq!(repository_name("Master Thesis"), "Master-Thesis");
        assert_eq!(repository_name("paper_v2.1"), "paper_v2.1");
        assert_eq!(repository_name("Übung!"), "bung");
        assert_eq!(repository_name("!!!"), "document");
    }

    #[test]
    fn commits_use_the_private_address() {
        let user = User { login: "ada".to_string(), id: 7, name: None };
        assert_eq!(user.noreply_email(), "7+ada@users.noreply.github.com");
    }
}
