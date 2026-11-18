//! Logins for HTTPS remotes. The editor stores a token per host in the
//! system keychain, and git asks the editor itself for it: started with
//! `GIT_ASKPASS`, the editor answers on stdout and exits without a window.

use crate::secrets;

const SERVICE: &str = "editor-git";
const LABEL: &str = "Editor git login";
/// What git gets as the user name when none was given. GitHub takes any.
const DEFAULT_USER: &str = "git";

/// The host in a prompt of git, such as `Password for 'https://ada@github.com': `.
fn host_of(prompt: &str) -> Option<String> {
    let start = prompt.find("'https://")? + "'https://".len();
    let rest = &prompt[start..];
    let authority = &rest[..rest.find(['\'', '/'])?];
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// The answer to a prompt of git, from the keychain.
pub fn answer(prompt: &str) -> Option<String> {
    let host = host_of(prompt)?;
    if prompt.starts_with("Username") {
        let user = secrets::get(SERVICE, &format!("{host}/user")).ok().flatten();
        Some(user.unwrap_or_else(|| DEFAULT_USER.to_string()))
    } else if prompt.starts_with("Password") {
        secrets::get(SERVICE, &host).ok().flatten()
    } else {
        None
    }
}

/// The token kept for a host, if any. Blocks.
pub fn token(host: &str) -> Option<String> {
    secrets::get(SERVICE, host).ok().flatten()
}

/// Keeps a token for a host, and the user name if there is one. Blocks.
pub fn store(host: &str, user: &str, token: &str) -> Result<(), String> {
    secrets::set(SERVICE, LABEL, host, token)?;
    if !user.trim().is_empty() {
        secrets::set(SERVICE, LABEL, &format!("{host}/user"), user.trim())?;
    }
    Ok(())
}

/// Runs as the askpass program of git, if that is why the editor started.
pub fn answer_if_asked() {
    if std::env::var_os(git::ASKPASS_ENV).is_none() {
        return;
    }
    let prompt = std::env::args().nth(1).unwrap_or_default();
    match answer(&prompt) {
        Some(answer) => {
            println!("{answer}");
            std::process::exit(0);
        }
        // Git then reports the missing login, which the editor turns into a question.
        None => std::process::exit(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_name_their_host() {
        assert_eq!(host_of("Password for 'https://ada@github.com': ").as_deref(), Some("github.com"));
        assert_eq!(host_of("Username for 'https://gitlab.kit.edu': ").as_deref(), Some("gitlab.kit.edu"));
        assert_eq!(host_of("Username for 'https://git.example.org/thesis.git': ").as_deref(), Some("git.example.org"));
        assert_eq!(host_of("Enter passphrase for key '/home/a/.ssh/id_ed25519': "), None);
        assert_eq!(answer("Something else"), None);
    }
}
