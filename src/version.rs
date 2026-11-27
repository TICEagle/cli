const CONSTRAINT_OPERATOR: &str = ">=";
const MAX_VERSION_LENGTH: usize = 24;
const HINT_LABELS: &[(&str, &str)] = &[
    ("a", "alpha"),
    ("alpha", "alpha"),
    ("b", "beta"),
    ("beta", "beta"),
    ("pre", "pre"),
    ("rc", "rc"),
];

pub fn release_version(input: &str) -> Result<String, String> {
    let version = parse_version(input)?;
    if version.len() > MAX_VERSION_LENGTH {
        return Err(format!(
            "version {version} is {} characters long, the calculator app can show at most {MAX_VERSION_LENGTH}",
            version.len()
        ));
    }
    Ok(version)
}

fn parse_version(input: &str) -> Result<String, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("the version is missing".into());
    }
    if text.contains('+') {
        return Err("build metadata like +build.5 is not allowed, because SemVer ignores it when comparing versions".into());
    }
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let (core, prerelease) = match text.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (text, None),
    };
    let mut parts: Vec<&str> = core.split('.').collect();
    let valid = parts.len() <= 3
        && parts.iter().all(|part| is_number(part))
        && prerelease.is_none_or(valid_prerelease);
    if !valid {
        return Err(version_hint(input, text));
    }
    for part in &parts {
        if part.parse::<i32>().is_err() {
            return Err(format!(
                "the version {input:?} has a number above 2147483647"
            ));
        }
    }
    parts.resize(3, "0");
    let mut normalized = parts.join(".");
    if let Some(prerelease) = prerelease {
        normalized.push('-');
        normalized.push_str(prerelease);
    }
    Ok(normalized)
}

fn is_number(part: &str) -> bool {
    !part.is_empty()
        && part.bytes().all(|byte| byte.is_ascii_digit())
        && (part == "0" || !part.starts_with('0'))
}

fn valid_prerelease(prerelease: &str) -> bool {
    prerelease.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (!identifier.bytes().all(|byte| byte.is_ascii_digit()) || is_number(identifier))
    })
}

fn version_hint(input: &str, text: &str) -> String {
    let fallback = format!("{input:?} is not a valid version, use major.minor.patch like 1.2.0");
    let Some(label_at) = text.find(|char: char| char.is_ascii_alphabetic()) else {
        return fallback;
    };
    let (prefix, rest) = text.split_at(label_at);
    let prefix = prefix.strip_suffix(['-', '.']).unwrap_or(prefix);
    let label_end = rest
        .find(|char: char| !char.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (label, number) = rest.split_at(label_end);
    let number = number.strip_prefix('.').unwrap_or(number);
    let core_parts: Vec<&str> = prefix.split('.').collect();
    let core_valid = core_parts.len() <= 3
        && core_parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    let label = HINT_LABELS
        .iter()
        .find(|(short, _)| short.eq_ignore_ascii_case(label));
    let (Some((_, label)), true, true) = (
        label,
        core_valid,
        number.bytes().all(|byte| byte.is_ascii_digit()),
    ) else {
        return fallback;
    };
    let suggestion = match parse_version(prefix) {
        Ok(suggestion) => suggestion,
        Err(error) => return error,
    };
    let mut hint = format!("{suggestion}-{label}");
    if !number.is_empty() {
        hint.push('.');
        hint.push_str(number);
    }
    format!("{input:?} is not a valid version, did you mean {hint:?}?")
}

pub fn check_constraint(input: &str) -> Result<(), String> {
    let text = input.trim();
    if text.is_empty() {
        return Ok(());
    }
    let Some(minimum) = text.strip_prefix(CONSTRAINT_OPERATOR) else {
        return Err(format!(
            "the dependency constraint {input:?} is not supported, use a minimum version like >=1.2.0"
        ));
    };
    parse_version(minimum).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_clear_versions() {
        assert_eq!(parse_version("v1.2").unwrap(), "1.2.0");
        assert_eq!(parse_version("2").unwrap(), "2.0.0");
        assert_eq!(parse_version("1.0.0-beta.1").unwrap(), "1.0.0-beta.1");
    }

    #[test]
    fn rejects_unclear_versions_with_a_hint() {
        assert_eq!(
            parse_version("1.0b").unwrap_err(),
            "\"1.0b\" is not a valid version, did you mean \"1.0.0-beta\"?"
        );
        assert_eq!(
            parse_version("2.1rc2").unwrap_err(),
            "\"2.1rc2\" is not a valid version, did you mean \"2.1.0-rc.2\"?"
        );
        assert!(
            parse_version("1.02")
                .unwrap_err()
                .contains("major.minor.patch")
        );
        assert!(parse_version("1.0.0-01").is_err());
        assert!(parse_version("1.2.3.4").is_err());
    }

    #[test]
    fn rejects_build_metadata_and_huge_numbers() {
        assert!(
            parse_version("1.0.0+build.5")
                .unwrap_err()
                .contains("build metadata")
        );
        assert!(
            parse_version("3000000000")
                .unwrap_err()
                .contains("2147483647")
        );
        assert_eq!(parse_version(" ").unwrap_err(), "the version is missing");
    }

    #[test]
    fn caps_release_versions_at_24_characters() {
        let fits = format!("1.0.0-{}", "a".repeat(18));
        assert_eq!(release_version(&fits).unwrap(), fits);
        assert!(
            release_version(&format!("1.0.0-{}", "a".repeat(19)))
                .unwrap_err()
                .contains("at most 24")
        );
    }

    #[test]
    fn checks_constraints() {
        assert!(check_constraint(">=1.2").is_ok());
        assert!(check_constraint("").is_ok());
        assert!(
            check_constraint("^1.0.0")
                .unwrap_err()
                .contains("not supported")
        );
    }
}
