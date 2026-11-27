use std::{collections::BTreeMap, fmt, fs, io, path::Path};

use serde::Deserialize;

use crate::version::check_constraint;

pub const CONFIG_FILE: &str = "ceagle.toml";
pub const ROLES: &[&str] = &["main", "data", "dev"];
pub const MAX_DESCRIPTION: usize = 200;
const MAX_DEPENDENCIES: usize = 64;
const MAX_SLUG: usize = 32;

#[derive(Debug)]
pub enum ConfigError {
    Read(io::Error),
    Parse(String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read(error) if error.kind() == io::ErrorKind::NotFound => write!(
                formatter,
                "there is no {CONFIG_FILE} in this folder, create one with project and files"
            ),
            ConfigError::Read(error) => write!(formatter, "failed to read {CONFIG_FILE}: {error}"),
            ConfigError::Parse(error) => write!(formatter, "{CONFIG_FILE} is not valid: {error}"),
            ConfigError::Invalid(problem) => write!(formatter, "{CONFIG_FILE}: {problem}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSettings {
    pub role: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project: String,
    pub files: Vec<String>,
    pub main: Option<String>,
    #[serde(default)]
    pub file: BTreeMap<String, FileSettings>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

pub fn valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (2..=MAX_SLUG).contains(&bytes.len())
        && bytes.first().is_some_and(|first| *first != b'-')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub fn normalize_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

impl Config {
    pub fn load(folder: &Path) -> Result<Config, ConfigError> {
        let text = fs::read_to_string(folder.join(CONFIG_FILE)).map_err(ConfigError::Read)?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let mut config: Config =
            toml::from_str(text).map_err(|error| ConfigError::Parse(error.to_string()))?;
        config.file = std::mem::take(&mut config.file)
            .into_iter()
            .map(|(path, settings)| (normalize_path(&path), settings))
            .collect();
        config.check().map_err(ConfigError::Invalid)?;
        Ok(config)
    }

    fn check(&self) -> Result<(), String> {
        if !valid_slug(&self.project) {
            return Err(format!(
                "project {:?} is not a CEagle project slug, slugs are 2 to 32 lowercase letters, digits and -",
                self.project
            ));
        }
        if self.files.is_empty() {
            return Err(
                "files is empty, list the files of the release like files = [\"bin/GAME.8xp\"]"
                    .into(),
            );
        }
        if self
            .main
            .as_deref()
            .is_some_and(|main| main.trim().is_empty())
        {
            return Err(
                "main is empty, give the name of the main variable like main = \"GAME\"".into(),
            );
        }
        for (path, settings) in &self.file {
            if let Some(role) = &settings.role
                && !ROLES.contains(&role.as_str())
            {
                return Err(format!(
                    "the role of {path} must be main, data or dev, not {role:?}"
                ));
            }
            if let Some(description) = &settings.description
                && description.chars().count() > MAX_DESCRIPTION
            {
                return Err(format!(
                    "the description of {path} is longer than {MAX_DESCRIPTION} characters"
                ));
            }
        }
        if self.dependencies.len() > MAX_DEPENDENCIES {
            return Err(format!(
                "a release can have at most {MAX_DEPENDENCIES} dependencies"
            ));
        }
        for (slug, constraint) in &self.dependencies {
            if !valid_slug(slug) {
                return Err(format!(
                    "the dependency {slug:?} is not a CEagle project slug"
                ));
            }
            check_constraint(constraint)
                .map_err(|error| format!("the dependency on {slug} is not valid: {error}"))?;
        }
        Ok(())
    }

    pub fn settings(&self, path: &str) -> Option<&FileSettings> {
        self.file.get(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
project = "gdash"
files   = ["bin/GDASH.8xp", "bin/GDLVL*.8xv", "bin/GDSAVE.8xv"]
main    = "GDASH"

[file."./bin/GDSAVE.8xv"]
description = "Save data"

[dependencies]
gdash-levels = ">=1.0.0"
"#;

    fn problem(text: &str) -> String {
        Config::parse(text).unwrap_err().to_string()
    }

    #[test]
    fn parses_the_documented_example() {
        let config = Config::parse(EXAMPLE).unwrap();
        assert_eq!(config.project, "gdash");
        assert_eq!(config.main.as_deref(), Some("GDASH"));
        assert_eq!(config.files.len(), 3);
        let settings = config.settings("bin/GDSAVE.8xv").unwrap();
        assert_eq!(settings.description.as_deref(), Some("Save data"));
        assert_eq!(config.dependencies["gdash-levels"], ">=1.0.0");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(
            problem("project = \"gdash\"\nfiles = [\"A.8xp\"]\nversion = \"1\"")
                .contains("unknown field")
        );
        assert!(
            problem("project = \"gdash\"\nfiles = [\"A.8xp\"]\n[file.\"A.8xp\"]\nname = \"x\"")
                .contains("unknown field")
        );
    }

    #[test]
    fn rejects_bad_values() {
        assert!(problem("project = \"GD\"\nfiles = [\"A.8xp\"]").contains("slug"));
        assert!(problem("project = \"gdash\"\nfiles = []").contains("files is empty"));
        assert!(
            problem("project = \"gdash\"\nfiles = [\"A.8xp\"]\n[file.\"A.8xp\"]\nrole = \"lib\"")
                .contains("main, data or dev")
        );
        assert!(
            problem("project = \"gdash\"\nfiles = [\"A.8xp\"]\n[dependencies]\ngraphx = \"^1\"")
                .contains("not supported")
        );
        assert!(problem("project = \"gdash\"\nfiles = \"A.8xp\"").contains("not valid"));
        assert!(problem("files = [\"A.8xp\"]").contains("missing field"));
    }
}
