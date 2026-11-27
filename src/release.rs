use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::Path,
};

use crate::{
    config::{Config, MAX_DESCRIPTION, normalize_path},
    glob,
    variable::{self, MAX_FILE_SIZE, TYPE_PROGRAM, TYPE_PROTECTED_PROGRAM},
};

const MAX_FILES: usize = 64;
const MAX_TOTAL_BYTES: usize = 3 << 20;
const MAX_DEV_FILE_BYTES: usize = 64 << 10;
const MAX_FILENAME: usize = 64;
const MAX_VARIABLE_BYTES: usize = 65504;
const SIZE_WORD_BYTES: usize = 2;
const CALCULATOR_EXTENSIONS: &[&str] = &[".8xp", ".8xv", ".8xl", ".8xm", ".8xs", ".8ci", ".8ca"];
const DEV_EXTENSIONS: &[&str] = &[".h", ".lib", ".txt", ".md"];

#[derive(Debug)]
pub struct LocalFile {
    pub path: String,
    pub filename: String,
    pub content: Vec<u8>,
    pub role: String,
    pub description: String,
    pub detected: String,
    pub variable_name: String,
    claim: Option<(u8, Vec<u8>)>,
}

#[derive(Debug)]
pub struct Release {
    pub files: Vec<LocalFile>,
    pub problems: Vec<String>,
}

fn extension(filename: &str) -> String {
    filename
        .rfind('.')
        .map(|at| filename[at..].to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_calculator_file(filename: &str) -> bool {
    CALCULATOR_EXTENSIONS.contains(&extension(filename).as_str())
}

fn file_limit(filename: &str) -> usize {
    if is_calculator_file(filename) {
        MAX_FILE_SIZE
    } else {
        MAX_DEV_FILE_BYTES
    }
}

fn too_large(filename: &str) -> String {
    if is_calculator_file(filename) {
        format!(
            "{filename} is larger than a calculator file can be, the calculator can hold at most 65,504 bytes of variable data"
        )
    } else {
        format!("{filename} is larger than 64 KiB, the limit for dev files")
    }
}

fn valid_filename(filename: &str) -> bool {
    filename.len() <= MAX_FILENAME
        && filename
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn read_limited(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut content = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut content)?;
    Ok(content)
}

fn detect(file: &mut LocalFile) -> Result<(), String> {
    let filename = &file.filename;
    if !valid_filename(filename) {
        return Err(format!(
            "{filename:?} is not a valid file name, use up to 64 letters, digits, ., _ and -"
        ));
    }
    if file.content.len() > file_limit(filename) {
        return Err(too_large(filename));
    }
    if !is_calculator_file(filename) {
        if !DEV_EXTENSIONS.contains(&extension(filename).as_str()) {
            return Err(format!(
                "{filename} is not a calculator file or a .h, .lib, .txt or .md dev file"
            ));
        }
        if std::str::from_utf8(&file.content).is_err() || file.content.contains(&0) {
            return Err(format!(
                "{filename} is not UTF-8 text, dev files must be text"
            ));
        }
        file.detected = "dev file".into();
        return Ok(());
    }
    let parsed = variable::parse(&file.content)
        .map_err(|error| format!("{filename} is not a valid TI variable file: {error}"))?;
    let wanted = variable::extension(parsed.kind).unwrap_or_default();
    if wanted != extension(filename) {
        return Err(format!(
            "{filename} holds a {}, so its extension must be {wanted}",
            variable::type_name(parsed.kind)
        ));
    }
    if parsed.data.len() > MAX_VARIABLE_BYTES + SIZE_WORD_BYTES {
        return Err(too_large(filename));
    }
    file.detected = match parsed.library_version() {
        Some(version) => format!("LibLoad library, version {version}"),
        None if parsed.is_asm() => "ASM program".into(),
        None if parsed.is_basic() => "TI-BASIC program".into(),
        None => variable::type_name(parsed.kind),
    };
    file.variable_name = parsed.display_name();
    let claim_kind = if parsed.kind == TYPE_PROTECTED_PROGRAM {
        TYPE_PROGRAM
    } else {
        parsed.kind
    };
    file.claim = Some((claim_kind, parsed.name.to_vec()));
    Ok(())
}

pub fn load(folder: &Path, config: &Config) -> Result<Release, String> {
    let mut paths: Vec<String> = Vec::new();
    for pattern in &config.files {
        let found = glob::expand(folder, pattern)
            .map_err(|error| format!("failed to list the files for {pattern}: {error}"))?;
        if found.is_empty() {
            return Err(if glob::has_wildcard(pattern) {
                format!("no file matches {pattern}")
            } else {
                format!("{pattern} does not exist")
            });
        }
        for path in found {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    if paths.len() > MAX_FILES {
        return Err(format!(
            "a release can have at most {MAX_FILES} files, the patterns in files match {}",
            paths.len()
        ));
    }
    let mut release = Release {
        files: Vec::new(),
        problems: Vec::new(),
    };
    for path in &paths {
        let filename = path.rsplit('/').next().unwrap_or(path).to_string();
        let content = read_limited(&folder.join(path), file_limit(&filename))
            .map_err(|error| format!("failed to read {path}: {error}"))?;
        let settings = config.settings(path);
        let mut file = LocalFile {
            path: path.clone(),
            filename,
            content,
            role: settings
                .and_then(|settings| settings.role.clone())
                .unwrap_or_default(),
            description: settings
                .and_then(|settings| settings.description.clone())
                .unwrap_or_default(),
            detected: "not readable".into(),
            variable_name: String::new(),
            claim: None,
        };
        if let Err(problem) = detect(&mut file) {
            release.problems.push(problem);
        }
        release.files.push(file);
    }
    for path in config.file.keys() {
        if !paths.contains(&normalize_path(path)) {
            release.problems.push(format!(
                "there are settings for {path}, but files does not include it"
            ));
        }
    }
    if let Some(main) = &config.main {
        assign_main(&mut release, main);
    }
    check(&mut release);
    Ok(release)
}

fn assign_main(release: &mut Release, main: &str) {
    let holders: Vec<usize> = release
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.variable_name == main)
        .map(|(index, _)| index)
        .collect();
    match holders.as_slice() {
        [] => release.problems.push(format!("main is {main}, but no calculator file holds a variable with that name")),
        [index] => {
            let file = &mut release.files[*index];
            if file.role.is_empty() || file.role == "main" {
                file.role = "main".into();
            } else {
                release.problems.push(format!("{} is main, but its settings give it the role {}", file.path, file.role));
            }
        }
        _ => release.problems.push(format!(
            "several files hold a variable named {main}, set role = \"main\" in the settings of one of them instead"
        )),
    }
}

fn check(release: &mut Release) {
    let mut names: HashMap<String, &str> = HashMap::new();
    let mut claims: HashMap<&(u8, Vec<u8>), &str> = HashMap::new();
    let mut total = 0;
    let mut mains = 0;
    for file in &release.files {
        total += file.content.len();
        if names
            .insert(file.filename.to_lowercase(), &file.path)
            .is_some()
        {
            release.problems.push(format!(
                "{} appears more than once, file names must be unique",
                file.filename
            ));
        }
        if file.description.chars().count() > MAX_DESCRIPTION {
            release.problems.push(format!(
                "the description of {} is longer than {MAX_DESCRIPTION} characters",
                file.path
            ));
        }
        let calculator = is_calculator_file(&file.filename);
        match file.role.as_str() {
            "main" if !calculator => release.problems.push(format!(
                "{} is not a calculator file, so it can't be the main file",
                file.path
            )),
            "data" if !calculator => release.problems.push(format!(
                "{} is not a calculator file, so its role must be dev",
                file.path
            )),
            "dev" if calculator => release.problems.push(format!(
                "{} is a calculator file, so its role must be main or data",
                file.path
            )),
            _ => {}
        }
        if file.role == "main" {
            mains += 1;
        }
        if let Some(claim) = &file.claim
            && let Some(other) = claims.insert(claim, &file.path)
        {
            release.problems.push(format!(
                "{other} and {} both hold the variable {}",
                file.path, file.variable_name
            ));
        }
    }
    if total > MAX_TOTAL_BYTES {
        release
            .problems
            .push("a release can be at most 3 MiB in total".into());
    }
    if mains > 1 {
        release
            .problems
            .push("the release has more than one main file".into());
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::variable::{TYPE_APPVAR, build, sized};

    fn folder(name: &str, files: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("ceagle-release-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, content) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        root
    }

    fn program(name: &[u8]) -> Vec<u8> {
        build(TYPE_PROGRAM, name, &sized(&[0xDE, 0x2A, 0x48, 0x2A]))
    }

    fn appvar(name: &[u8]) -> Vec<u8> {
        build(TYPE_APPVAR, name, &sized(&[1, 2, 3]))
    }

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn detects_files_and_assigns_main() {
        let root = folder(
            "good",
            &[
                ("bin/GDASH.8xp", program(b"GDASH")),
                ("bin/GDLVL1.8xv", appvar(b"GDLVL1")),
                ("bin/GDLVL2.8xv", appvar(b"GDLVL2")),
                ("README.md", b"# Geometry Dash".to_vec()),
            ],
        );
        let config = config(
            "project = \"gdash\"\nfiles = [\"bin/*.8xp\", \"bin/GDLVL*.8xv\", \"README.md\"]\nmain = \"GDASH\"\n[file.\"README.md\"]\ndescription = \"Docs\"",
        );
        let release = load(&root, &config).unwrap();
        assert!(release.problems.is_empty(), "{:?}", release.problems);
        let summary: Vec<_> = release
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.detected.as_str(),
                    file.role.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("bin/GDASH.8xp", "TI-BASIC program", "main"),
                ("bin/GDLVL1.8xv", "appvar", ""),
                ("bin/GDLVL2.8xv", "appvar", ""),
                ("README.md", "dev file", ""),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_broken_and_conflicting_files() {
        let mut bad_checksum = program(b"GDASH");
        let last = bad_checksum.len() - 1;
        bad_checksum[last] ^= 0xFF;
        let root = folder(
            "bad",
            &[
                ("a/BROKEN.8xp", bad_checksum),
                ("a/SAME.8xv", appvar(b"SAME")),
                ("b/SAME.8xv", appvar(b"SAME")),
                ("a/WRONG.8xl", appvar(b"WRONG")),
                ("a/NOTES.txt", vec![0xFF, 0x00]),
                ("a/TOO.txt", vec![b'a'; MAX_DEV_FILE_BYTES + 1]),
            ],
        );
        let config = config(
            "project = \"gdash\"\nfiles = [\"a/*\", \"b/*\"]\nmain = \"NOPE\"\n[file.\"a/SAME.8xv\"]\nrole = \"dev\"\n[file.\"c/X.8xp\"]\nrole = \"data\"",
        );
        let release = load(&root, &config).unwrap();
        let problems = release.problems.join("\n");
        for expected in [
            "BROKEN.8xp is not a valid TI variable file: the checksum does not match",
            "WRONG.8xl holds a appvar, so its extension must be .8xv",
            "NOTES.txt is not UTF-8 text",
            "TOO.txt is larger than 64 KiB",
            "settings for c/X.8xp",
            "main is NOPE",
            "SAME.8xv appears more than once",
            "a/SAME.8xv is a calculator file, so its role must be main or data",
            "a/SAME.8xv and b/SAME.8xv both hold the variable SAME",
        ] {
            assert!(
                problems.contains(expected),
                "missing {expected:?} in\n{problems}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caps_variable_data_at_the_calculator_limit() {
        let appvar_of =
            |name: &[u8], length: usize| build(TYPE_APPVAR, name, &sized(&vec![0; length]));
        let root = folder(
            "limit",
            &[
                ("FITS.8xv", appvar_of(b"FITS", MAX_VARIABLE_BYTES)),
                ("OVER.8xv", appvar_of(b"OVER", MAX_VARIABLE_BYTES + 1)),
            ],
        );
        let release = load(&root, &config("project = \"big\"\nfiles = [\"*.8xv\"]")).unwrap();
        assert_eq!(
            release.problems,
            [
                "OVER.8xv is larger than a calculator file can be, the calculator can hold at most 65,504 bytes of variable data"
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fails_when_a_pattern_matches_nothing() {
        let root = folder("empty", &[("bin/GDASH.8xp", program(b"GDASH"))]);
        let error = load(
            &root,
            &config("project = \"gdash\"\nfiles = [\"bin/*.8xv\"]"),
        )
        .unwrap_err();
        assert_eq!(error, "no file matches bin/*.8xv");
        let error = load(
            &root,
            &config("project = \"gdash\"\nfiles = [\"bin/GD.8xp\"]"),
        )
        .unwrap_err();
        assert_eq!(error, "bin/GD.8xp does not exist");
        fs::remove_dir_all(root).unwrap();
    }
}
