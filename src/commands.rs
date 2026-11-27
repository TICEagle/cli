use std::{
    collections::HashMap,
    fmt::Write,
    fs::{self, File},
    io::Read,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use ring::digest::{SHA256, digest};

use crate::{
    api::{ApiError, Client, ReleaseResponse, Upload},
    auth::{self, TOKEN_VARIABLE},
    config::{Config, ConfigError},
    release::{self, LocalFile, Release},
    report,
    version::release_version,
};

const MISSING_VERSION: &str = "the version is missing";
const MAX_NOTES_BYTES: usize = 32 << 10;
const MAX_ATTESTATION_BYTES: u64 = 128 << 10;
const PROCESSING: &str = "processing";

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Error(String),
    Problems,
    UnderReview,
    Rejected,
    StillProcessing,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub api: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub attestation: Option<String>,
}

pub struct Waiting {
    pub interval: Duration,
    pub timeout: Duration,
}

pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn fail(problems: &[String]) -> Failure {
    print!("\n{}", report::problems(problems));
    Failure::Problems
}

fn explicit_role(file: &LocalFile) -> String {
    if file.role.is_empty() {
        "auto".into()
    } else {
        file.role.clone()
    }
}

fn read_notes(path: &str) -> Result<String, Failure> {
    let notes = fs::read_to_string(path).map_err(|error| {
        Failure::Error(format!("failed to read the release notes {path}: {error}"))
    })?;
    if notes.len() > MAX_NOTES_BYTES {
        return Err(fail(&[format!(
            "the release notes in {path} are longer than 32 KiB"
        )]));
    }
    Ok(notes)
}

fn read_attestation(path: &str) -> Result<Vec<u8>, Failure> {
    let mut content = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(MAX_ATTESTATION_BYTES + 1)
                .read_to_end(&mut content)
        })
        .map_err(|error| {
            Failure::Error(format!("failed to read the attestation {path}: {error}"))
        })?;
    if content.len() as u64 > MAX_ATTESTATION_BYTES {
        return Err(fail(&[format!(
            "the attestation in {path} is larger than 128 KiB"
        )]));
    }
    Ok(content)
}

fn load(folder: &Path) -> Result<(Config, Release), Failure> {
    let config = Config::load(folder).map_err(|error| match error {
        ConfigError::Read(_) => Failure::Error(error.to_string()),
        _ => {
            println!("{error}");
            Failure::Problems
        }
    })?;
    let release = release::load(folder, &config).map_err(|problem| fail(&[problem]))?;
    Ok((config, release))
}

pub fn checksums(folder: &Path) -> Result<String, Failure> {
    let (_, release) = load(folder)?;
    let mut lines = String::new();
    for file in &release.files {
        for byte in digest(&SHA256, &file.content).as_ref() {
            let _ = write!(lines, "{byte:02x}");
        }
        let _ = writeln!(lines, "  {}", file.filename);
    }
    Ok(lines)
}

fn prepare(folder: &Path) -> Result<(Config, Release), Failure> {
    let (config, release) = load(folder)?;
    let noun = if release.files.len() == 1 {
        "file"
    } else {
        "files"
    };
    println!(
        "Checked {} {noun} of {}.\n",
        release.files.len(),
        config.project
    );
    Ok((config, release))
}

fn token(client: &Client, project: &str, lookup: Lookup) -> Result<Option<auth::Token>, Failure> {
    auth::find(client, project, &lookup).map_err(Failure::Error)
}

pub fn validate(folder: &Path, options: &Options, lookup: Lookup) -> Result<(), Failure> {
    let (config, release) = prepare(folder)?;
    let mut problems = release.problems.clone();
    let version = options.version.as_deref().map(release_version).transpose();
    if let Err(problem) = &version {
        problems.push(problem.clone());
    }
    let notes = options
        .notes
        .as_deref()
        .map(read_notes)
        .transpose()?
        .unwrap_or_default();
    if !problems.is_empty() {
        print!("{}", report::file_table(&release.files, explicit_role));
        return Err(fail(&problems));
    }
    let client = Client::new(&options.api);
    let Some(token) = token(&client, &config.project, lookup)? else {
        print!("{}", report::file_table(&release.files, explicit_role));
        println!(
            "\nThe files look right. Run ceagle login or set {TOKEN_VARIABLE} to also run the checks CEagle does for {}.",
            config.project
        );
        return Ok(());
    };
    let version = version.ok().flatten().unwrap_or_default();
    let upload = Upload {
        version: &version,
        notes: &notes,
        files: &release.files,
        dependencies: &config.dependencies,
        attestation: None,
    };
    let answer = client
        .dry_run(&config.project, &token.value, &upload)
        .map_err(|error| {
            println!("{}", report::file_table(&release.files, explicit_role));
            Failure::Error(format!("CEagle could not check the release: {error}"))
        })?;
    let roles: HashMap<&str, &str> = answer
        .files
        .iter()
        .map(|file| (file.filename.as_str(), file.role.as_str()))
        .collect();
    print!(
        "{}",
        report::file_table(&release.files, |file| roles
            .get(file.filename.as_str())
            .map_or_else(|| explicit_role(file), |role| role.to_string()))
    );
    if let Some(dependencies) = report::dependencies(&answer.dependencies) {
        println!("\n{dependencies}");
    }
    let problems: Vec<String> = answer
        .problems
        .unwrap_or_default()
        .into_iter()
        .filter(|problem| !(version.is_empty() && problem == MISSING_VERSION))
        .collect();
    if !problems.is_empty() {
        return Err(fail(&problems));
    }
    let checked = if answer.version.is_empty() {
        String::new()
    } else {
        format!(" {}", answer.version)
    };
    println!(
        "\nCEagle checked {}{checked} with {} and found no problems.",
        config.project, token.via
    );
    Ok(())
}

pub fn publish(
    folder: &Path,
    options: &Options,
    lookup: Lookup,
    waiting: &Waiting,
) -> Result<(), Failure> {
    let (config, release) = prepare(folder)?;
    print!("{}", report::file_table(&release.files, explicit_role));
    let mut problems = release.problems.clone();
    let version = match release_version(options.version.as_deref().unwrap_or_default()) {
        Ok(version) => version,
        Err(problem) => {
            problems.push(problem);
            String::new()
        }
    };
    let notes = options
        .notes
        .as_deref()
        .map(read_notes)
        .transpose()?
        .unwrap_or_default();
    let attestation = options
        .attestation
        .as_deref()
        .map(read_attestation)
        .transpose()?;
    if !problems.is_empty() {
        return Err(fail(&problems));
    }
    let client = Client::new(&options.api);
    let token = token(&client, &config.project, lookup)?.ok_or_else(|| {
        Failure::Error(format!(
            "publishing needs a token: run ceagle login, set {TOKEN_VARIABLE}, or run in GitHub Actions with permissions: id-token: write"
        ))
    })?;
    let attested = if attestation.is_some() {
        " and its attestation"
    } else {
        ""
    };
    println!(
        "\nUploading {} {version}{attested} with {}.",
        config.project, token.via
    );
    let upload = Upload {
        version: &version,
        notes: &notes,
        files: &release.files,
        dependencies: &config.dependencies,
        attestation: attestation.as_deref(),
    };
    let accepted = client
        .publish(&config.project, &token.value, &upload)
        .map_err(|error| match error {
            ApiError::Problems(problems) => fail(&problems),
            error => Failure::Error(format!("CEagle did not accept the release: {error}")),
        })?;
    let page = format!("{}/p/{}", client.base(), config.project);
    let finished = wait(&client, &config.project, &token.value, accepted, waiting)?;
    outcome(&config.project, &page, finished, waiting)
}

fn wait(
    client: &Client,
    project: &str,
    token: &str,
    mut release: ReleaseResponse,
    waiting: &Waiting,
) -> Result<ReleaseResponse, Failure> {
    let started = Instant::now();
    if release.status == PROCESSING {
        println!("Processing {project} {}...", release.version);
    }
    while release.status == PROCESSING && started.elapsed() < waiting.timeout {
        thread::sleep(waiting.interval);
        match client.release(project, &release.version, token) {
            Ok(latest) => release = latest,
            Err(ApiError::Network(error)) => eprintln!("{error}, trying again"),
            Err(error) => {
                return Err(Failure::Error(format!(
                    "failed to check on {project} {}: {error}",
                    release.version
                )));
            }
        }
    }
    Ok(release)
}

fn outcome(
    project: &str,
    page: &str,
    release: ReleaseResponse,
    waiting: &Waiting,
) -> Result<(), Failure> {
    let name = format!("{project} {}", release.version);
    let capabilities = report::capabilities(&release.capabilities.unwrap_or_default());
    let reason = release
        .reason
        .map(|reason| format!("\nReason: {reason}"))
        .unwrap_or_default();
    match release.status.as_str() {
        "published" => {
            println!("Published {name}: {page}\n{capabilities}");
            Ok(())
        }
        "held" => {
            println!(
                "Under review: {name} is waiting for a CEagle admin before it is published. You get an email when they decide.{reason}\n{capabilities}"
            );
            Err(Failure::UnderReview)
        }
        "rejected" => {
            println!("Rejected: {name} was not published.{reason}");
            Err(Failure::Rejected)
        }
        PROCESSING => {
            println!(
                "{name} is still processing after {} minutes. CEagle keeps working on it, check {page} later.",
                waiting.timeout.as_secs() / 60
            );
            Err(Failure::StillProcessing)
        }
        other => Err(Failure::Error(format!(
            "{name} has the unexpected status {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        api::fake::serve,
        variable::{TYPE_PROGRAM, build, sized},
    };

    const QUICK: Waiting = Waiting {
        interval: Duration::from_millis(10),
        timeout: Duration::from_secs(5),
    };
    const PROCESSING_JSON: &str =
        r#"{"version":"1.2.0","status":"processing","capabilities":null}"#;

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ceagle-commands-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join("bin/GDASH.8xp"),
            build(TYPE_PROGRAM, b"GDASH", &sized(&[0xDE, 0x2A])),
        )
        .unwrap();
        fs::write(root.join("ceagle.toml"), "project = \"gdash\"\nfiles = [\"bin/*.8xp\"]\nmain = \"GDASH\"\n[dependencies]\ngraphx = \">=1\"").unwrap();
        root
    }

    fn options(base: &str, version: Option<&str>) -> Options {
        Options {
            api: base.into(),
            version: version.map(String::from),
            notes: None,
            attestation: None,
        }
    }

    fn with_token(name: &str) -> Option<String> {
        (name == TOKEN_VARIABLE).then(|| "ceagle_TEST".to_string())
    }

    #[test]
    fn publishes_and_waits_until_published() {
        let root = project("published");
        let (base, server) = serve(vec![
            (202, PROCESSING_JSON.into()),
            (200, PROCESSING_JSON.into()),
            (
                200,
                r#"{"version":"1.2.0","status":"published","capabilities":["usb"]}"#.into(),
            ),
        ]);
        assert_eq!(
            publish(&root, &options(&base, Some("v1.2")), &with_token, &QUICK),
            Ok(())
        );
        let requests = server.join().unwrap();
        assert!(
            requests[0]
                .head
                .starts_with("POST /api/v1/projects/gdash/releases ")
        );
        assert!(requests[0].head.contains("Bearer ceagle_TEST"));
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(body.contains(r#"{"version":"1.2.0","notes":"","files":[{"filename":"GDASH.8xp","role":"main","description":""}],"dependencies":{"graphx":">=1"}}"#));
        assert!(body.contains("name=\"file\"; filename=\"GDASH.8xp\""));
        assert!(
            requests[2]
                .head
                .starts_with("GET /api/v1/projects/gdash/releases/1.2.0 ")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_held_rejected_and_refused_releases() {
        let root = project("held");
        let (base, server) = serve(vec![
            (202, r#"{"version":"1.2.0","status":"held","capabilities":["flash_write"]}"#.into()),
            (202, r#"{"version":"1.2.0","status":"rejected","reason":"not a real program"}"#.into()),
            (422, r#"{"error":"version 1.2.0 of gdash already exists","problems":["version 1.2.0 of gdash already exists"]}"#.into()),
            (401, r#"{"error":"this API token is wrong, expired or revoked"}"#.into()),
        ]);
        let options = options(&base, Some("1.2.0"));
        assert_eq!(
            publish(&root, &options, &with_token, &QUICK),
            Err(Failure::UnderReview)
        );
        assert_eq!(
            publish(&root, &options, &with_token, &QUICK),
            Err(Failure::Rejected)
        );
        assert_eq!(
            publish(&root, &options, &with_token, &QUICK),
            Err(Failure::Problems)
        );
        assert!(
            matches!(publish(&root, &options, &with_token, &QUICK), Err(Failure::Error(error)) if error.contains("revoked"))
        );
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publish_sends_the_attestation() {
        let root = project("attested");
        let bundle = root.join("bundle.json");
        fs::write(&bundle, r#"{"mediaType":"sigstore"}"#).unwrap();
        let (base, server) = serve(vec![(
            202,
            r#"{"version":"1.2.0","status":"published","capabilities":[]}"#.into(),
        )]);
        let options = Options {
            attestation: Some(bundle.display().to_string()),
            ..options(&base, Some("1.2.0"))
        };
        assert_eq!(publish(&root, &options, &with_token, &QUICK), Ok(()));
        let body = String::from_utf8_lossy(&server.join().unwrap()[0].body).into_owned();
        assert!(body.contains("name=\"attestation\"; filename=\"attestation.sigstore.json\"\r\nContent-Type: application/json\r\n\r\n{\"mediaType\":\"sigstore\"}\r\n--"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_missing_or_oversized_attestation_before_uploading() {
        let root = project("oversized");
        let bundle = root.join("bundle.json");
        fs::write(&bundle, vec![b' '; MAX_ATTESTATION_BYTES as usize + 1]).unwrap();
        let attested = |path: &Path| Options {
            attestation: Some(path.display().to_string()),
            ..options("http://127.0.0.1:9", Some("1.2.0"))
        };
        assert_eq!(
            publish(&root, &attested(&bundle), &with_token, &QUICK),
            Err(Failure::Problems)
        );
        assert!(matches!(
            publish(&root, &attested(&root.join("missing.json")), &with_token, &QUICK),
            Err(Failure::Error(error)) if error.contains("failed to read the attestation")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checksums_lists_every_release_file() {
        let root = project("checksums");
        fs::write(root.join("NOTES.md"), "abc").unwrap();
        fs::write(
            root.join("ceagle.toml"),
            "project = \"gdash\"\nfiles = [\"NOTES.md\"]\n",
        )
        .unwrap();
        assert_eq!(
            checksums(&root),
            Ok(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  NOTES.md\n"
                    .into()
            )
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stops_waiting_after_the_timeout() {
        let root = project("slow");
        let (base, _server) = serve(vec![
            (202, PROCESSING_JSON.into()),
            (200, PROCESSING_JSON.into()),
        ]);
        let waiting = Waiting {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(10),
        };
        assert_eq!(
            publish(&root, &options(&base, Some("1.2.0")), &with_token, &waiting),
            Err(Failure::StillProcessing)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_bad_versions_before_uploading() {
        let root = project("version");
        assert_eq!(
            publish(
                &root,
                &options("http://127.0.0.1:9", Some("1.0b")),
                &with_token,
                &QUICK
            ),
            Err(Failure::Problems)
        );
        assert_eq!(
            publish(
                &root,
                &options("http://127.0.0.1:9", None),
                &with_token,
                &QUICK
            ),
            Err(Failure::Problems)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validate_runs_the_dry_run_and_ignores_the_missing_version() {
        let root = project("validate");
        let clean = r#"{"version":"","libload_version":null,"files":[{"filename":"GDASH.8xp","role":"main"}],"dependencies":[],"problems":["the version is missing"]}"#;
        let broken = r#"{"version":"1.0.0","files":[],"dependencies":[{"slug":"graphx","min_version":"1.0.0","min_libload_version":null,"auto":false}],"problems":["GDASH.8xp can't be the main file of a asm project"]}"#;
        let (base, server) = serve(vec![(200, clean.into()), (200, broken.into())]);
        assert_eq!(validate(&root, &options(&base, None), &with_token), Ok(()));
        assert_eq!(
            validate(&root, &options(&base, Some("1")), &with_token),
            Err(Failure::Problems)
        );
        let requests = server.join().unwrap();
        assert!(
            requests[0]
                .head
                .starts_with("POST /api/v1/projects/gdash/releases?dry_run=1 ")
        );
        assert_eq!(
            validate(&root, &options("http://127.0.0.1:9", None), &|_| None),
            Ok(())
        );
        fs::remove_dir_all(root).unwrap();
    }
}
