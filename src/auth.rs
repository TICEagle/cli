use std::{
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

use crate::api::Client;

pub const TOKEN_VARIABLE: &str = "CEAGLE_TOKEN";
const REQUEST_URL_VARIABLE: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
const REQUEST_TOKEN_VARIABLE: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";
const AUDIENCE: &str = "ceagle.cc";
const FOLDER: &str = "ceagle";
const TOKEN_FILE: &str = "token";
const TOKEN_PREFIX: &str = "ceagle_";

pub struct Token {
    pub value: String,
    pub via: &'static str,
}

pub fn token_path(lookup: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| {
        lookup(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        set("APPDATA")
    } else if cfg!(target_os = "macos") {
        set("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        set("XDG_CONFIG_HOME").or_else(|| set("HOME").map(|home| home.join(".config")))
    };
    base.map(|base| base.join(FOLDER).join(TOKEN_FILE))
}

#[cfg(unix)]
pub fn save(path: &Path, token: &str) -> io::Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    };

    if let Some(folder) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(token.as_bytes())
}

#[cfg(not(unix))]
pub fn save(path: &Path, token: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(path, token)
}

pub fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn stored(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim().to_string()).filter(|token| !token.is_empty())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to read the saved token at {}: {error}",
            path.display()
        )),
    }
}

pub fn find(
    client: &Client,
    project: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Token>, String> {
    let set = |name: &str| {
        lookup(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    if let Some(value) = set(TOKEN_VARIABLE) {
        return Ok(Some(Token {
            value,
            via: TOKEN_VARIABLE,
        }));
    }
    if let Some(path) = token_path(lookup)
        && let Some(value) = stored(&path)?
    {
        return Ok(Some(Token {
            value,
            via: "the saved token",
        }));
    }
    let (Some(url), Some(request_token)) = (set(REQUEST_URL_VARIABLE), set(REQUEST_TOKEN_VARIABLE))
    else {
        return Ok(None);
    };
    let github = client
        .github_token(&url, &request_token, AUDIENCE)
        .map_err(|error| format!("failed to get a GitHub OIDC token: {error}"))?;
    let value = client
        .exchange(&github, project)
        .map_err(|error| format!("CEagle did not accept the GitHub OIDC token: {error}"))?;
    Ok(Some(Token {
        value,
        via: "GitHub Actions trusted publishing",
    }))
}

pub fn login(lookup: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let path = token_path(lookup)
        .ok_or_else(|| String::from("cannot find your home folder to save the token in"))?;
    let stdin = io::stdin();
    if stdin.is_terminal() {
        eprint!("Create an API token on your CEagle account page and paste it here: ");
        let _ = io::stderr().flush();
    }
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("failed to read the token: {error}"))?;
    let token = line.trim();
    if !token.starts_with(TOKEN_PREFIX)
        || token.len() == TOKEN_PREFIX.len()
        || token.contains(char::is_whitespace)
    {
        return Err(format!(
            "that is not a CEagle API token, tokens start with {TOKEN_PREFIX}"
        ));
    }
    save(&path, token)
        .map_err(|error| format!("failed to save the token to {}: {error}", path.display()))?;
    println!(
        "Saved the token to {}. {TOKEN_VARIABLE} still wins when it is set.",
        path.display()
    );
    Ok(())
}

pub fn logout(lookup: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let path = token_path(lookup).ok_or_else(|| String::from("cannot find your home folder"))?;
    match remove(&path) {
        Ok(true) => println!("Removed the saved token."),
        Ok(false) => println!("There was no saved token."),
        Err(error) => {
            return Err(format!("failed to remove {}: {error}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::api::fake::serve;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn temporary(name: &str) -> String {
        let folder =
            std::env::temp_dir().join(format!("ceagle-auth-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder.to_string_lossy().into_owned()
    }

    #[test]
    fn environment_token_wins_over_the_saved_one() {
        let home = temporary("env");
        let environment = lookup(&[
            ("XDG_CONFIG_HOME", &home),
            ("HOME", &home),
            ("APPDATA", &home),
            (TOKEN_VARIABLE, "ceagle_ENV"),
        ]);
        save(&token_path(&environment).unwrap(), "ceagle_SAVED").unwrap();
        let client = Client::new("http://127.0.0.1:9");
        let token = find(&client, "gdash", &environment).unwrap().unwrap();
        assert_eq!(token.value, "ceagle_ENV");
        let without = lookup(&[
            ("XDG_CONFIG_HOME", &home),
            ("HOME", &home),
            ("APPDATA", &home),
        ]);
        assert_eq!(
            find(&client, "gdash", &without).unwrap().unwrap().value,
            "ceagle_SAVED"
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn saved_token_is_private_and_removable() {
        use std::os::unix::fs::PermissionsExt;

        let home = temporary("private");
        let path = token_path(&lookup(&[("XDG_CONFIG_HOME", &home), ("HOME", &home)])).unwrap();
        save(&path, "ceagle_A").unwrap();
        save(&path, "ceagle_B").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "ceagle_B");
        assert!(remove(&path).unwrap());
        assert!(!remove(&path).unwrap());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn github_actions_tokens_are_exchanged_with_the_project() {
        let (base, server) = serve(vec![
            (200, r#"{"value":"github-jwt"}"#.into()),
            (
                200,
                r#"{"token":"ceagle_OIDC","project":"gdash","expires_at":"2026-11-26T12:00:00Z"}"#
                    .into(),
            ),
        ]);
        let home = temporary("oidc");
        let url = format!("{base}/oidc?api-version=2.0");
        let environment = lookup(&[
            ("XDG_CONFIG_HOME", &home),
            ("HOME", &home),
            ("APPDATA", &home),
            (REQUEST_URL_VARIABLE, &url),
            (REQUEST_TOKEN_VARIABLE, "secret"),
        ]);
        let token = find(&Client::new(&base), "gdash", &environment)
            .unwrap()
            .unwrap();
        assert_eq!(token.value, "ceagle_OIDC");
        let requests = server.join().unwrap();
        assert!(requests[0].head.contains("audience=ceagle.cc"));
        assert!(String::from_utf8_lossy(&requests[1].body).contains(r#""project":"gdash""#));
    }

    #[test]
    fn no_token_anywhere_is_not_an_error() {
        let home = temporary("none");
        let environment = lookup(&[
            ("XDG_CONFIG_HOME", &home),
            ("HOME", &home),
            ("APPDATA", &home),
        ]);
        assert!(
            find(&Client::new("http://127.0.0.1:9"), "gdash", &environment)
                .unwrap()
                .is_none()
        );
    }
}
