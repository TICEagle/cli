#![forbid(unsafe_code)]

mod api;
mod auth;
mod commands;
mod config;
mod glob;
mod name;
mod release;
mod report;
mod variable;
mod version;

use std::{env, path::Path, process::ExitCode, time::Duration};

use crate::commands::{Failure, Options, Waiting};

const DEFAULT_API: &str = "https://ceagle.cc";
const API_VARIABLE: &str = "CEAGLE_API";
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const POLL_TIMEOUT: Duration = Duration::from_secs(600);
const EXIT_ERROR: u8 = 1;
const EXIT_PROBLEMS: u8 = 2;
const EXIT_UNDER_REVIEW: u8 = 3;
const EXIT_REJECTED: u8 = 4;
const EXIT_STILL_PROCESSING: u8 = 5;
const USAGE: &str = "usage: ceagle validate [--version VERSION] [--notes FILE] [--api URL]
       ceagle publish --version VERSION [--notes FILE] [--api URL]
       ceagle login
       ceagle logout
       ceagle --version

ceagle reads ceagle.toml in the current folder. Tokens come from CEAGLE_TOKEN, the token saved
by ceagle login, or GitHub Actions trusted publishing. --api or CEAGLE_API picks another server.

exit codes: 0 done, 1 error, 2 problems to fix, 3 under review, 4 rejected, 5 still processing";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Validate(Options),
    Publish(Options),
    Login,
    Logout,
}

fn parse_options(arguments: &[String], default_api: String) -> Result<Options, String> {
    let mut options = Options {
        api: default_api,
        version: None,
        notes: None,
    };
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (argument.as_str(), None),
        };
        if !matches!(flag, "--version" | "--notes" | "--api") {
            return Err(format!("unknown option {argument}"));
        }
        let value = match inline.or_else(|| rest.next().cloned()) {
            Some(value) if !value.is_empty() => value,
            _ => return Err(format!("{flag} needs a value")),
        };
        match flag {
            "--version" => options.version = Some(value),
            "--notes" => options.notes = Some(value),
            _ => options.api = value,
        }
    }
    Ok(options)
}

fn parse_command(arguments: &[String], default_api: String) -> Result<Command, String> {
    let Some((first, rest)) = arguments.split_first() else {
        return Ok(Command::Help);
    };
    let bare = |command: Command| match rest {
        [] => Ok(command),
        [extra, ..] => Err(format!("unexpected argument {extra}")),
    };
    match first.as_str() {
        "help" | "--help" | "-h" => bare(Command::Help),
        "--version" | "-V" => bare(Command::Version),
        "validate" => parse_options(rest, default_api).map(Command::Validate),
        "publish" => {
            let options = parse_options(rest, default_api)?;
            if options.version.is_none() {
                return Err("publish needs --version, like ceagle publish --version 1.2.0".into());
            }
            Ok(Command::Publish(options))
        }
        "login" => bare(Command::Login),
        "logout" => bare(Command::Logout),
        other => Err(format!("unknown command {other}")),
    }
}

fn main() -> ExitCode {
    let lookup = |name: &str| env::var(name).ok();
    let arguments: Vec<String> = env::args().skip(1).collect();
    let default_api = lookup(API_VARIABLE)
        .filter(|api| !api.is_empty())
        .unwrap_or_else(|| DEFAULT_API.into());
    let command = match parse_command(&arguments, default_api) {
        Ok(command) => command,
        Err(problem) => {
            eprintln!("{problem}\n{USAGE}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let folder = Path::new(".");
    let waiting = Waiting {
        interval: POLL_INTERVAL,
        timeout: POLL_TIMEOUT,
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("ceagle {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Validate(options) => commands::validate(folder, &options, &lookup),
        Command::Publish(options) => commands::publish(folder, &options, &lookup, &waiting),
        Command::Login => auth::login(&lookup).map_err(Failure::Error),
        Command::Logout => auth::logout(&lookup).map_err(Failure::Error),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Error(error)) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_ERROR)
        }
        Err(Failure::Problems) => ExitCode::from(EXIT_PROBLEMS),
        Err(Failure::UnderReview) => ExitCode::from(EXIT_UNDER_REVIEW),
        Err(Failure::Rejected) => ExitCode::from(EXIT_REJECTED),
        Err(Failure::StillProcessing) => ExitCode::from(EXIT_STILL_PROCESSING),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Command, String> {
        let arguments: Vec<String> = line.split_whitespace().map(String::from).collect();
        parse_command(&arguments, DEFAULT_API.into())
    }

    #[test]
    fn parses_commands_and_options() {
        assert_eq!(
            parse("publish --version 1.2.0 --notes=NOTES.md --api http://127.0.0.1:8090"),
            Ok(Command::Publish(Options {
                api: "http://127.0.0.1:8090".into(),
                version: Some("1.2.0".into()),
                notes: Some("NOTES.md".into()),
            }))
        );
        assert_eq!(parse(""), Ok(Command::Help));
        assert_eq!(parse("--version"), Ok(Command::Version));
        assert!(
            matches!(parse("validate"), Ok(Command::Validate(options)) if options.api == DEFAULT_API)
        );
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(parse("publish").unwrap_err().contains("needs --version"));
        assert!(
            parse("publish --version")
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse("validate --force")
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(
            parse("login now")
                .unwrap_err()
                .contains("unexpected argument")
        );
        assert!(parse("deploy").unwrap_err().contains("unknown command"));
    }
}
