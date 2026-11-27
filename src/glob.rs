use std::{fs, io, path::Path};

const ANY_DEPTH: &str = "**";

pub fn has_wildcard(pattern: &str) -> bool {
    pattern.contains(['*', '?'])
}

pub fn expand(root: &Path, pattern: &str) -> io::Result<Vec<String>> {
    let segments: Vec<&str> = pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    let mut found = Vec::new();
    walk(root, String::new(), &segments, &mut found)?;
    found.sort();
    found.dedup();
    Ok(found)
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn walk(root: &Path, prefix: String, segments: &[&str], found: &mut Vec<String>) -> io::Result<()> {
    let Some((segment, rest)) = segments.split_first() else {
        if root.join(&prefix).is_file() {
            found.push(prefix);
        }
        return Ok(());
    };
    if !has_wildcard(segment) {
        return walk(root, join(&prefix, segment), rest, found);
    }
    let directory = root.join(&prefix);
    if !directory.is_dir() {
        return Ok(());
    }
    if *segment == ANY_DEPTH {
        walk(root, prefix.clone(), rest, found)?;
    }
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if *segment == ANY_DEPTH {
            if entry.file_type()?.is_dir() {
                walk(root, join(&prefix, &name), segments, found)?;
            }
        } else if matches(segment, &name) {
            walk(root, join(&prefix, &name), rest, found)?;
        }
    }
    Ok(())
}

pub fn matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut at, mut position) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while position < name.len() {
        match pattern.get(at) {
            Some('*') => {
                backtrack = Some((at, position));
                at += 1;
            }
            Some(&char) if char == '?' || char == name[position] => {
                at += 1;
                position += 1;
            }
            _ => match backtrack {
                Some((star, from)) => {
                    at = star + 1;
                    position = from + 1;
                    backtrack = Some((star, from + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at..].iter().all(|&char| char == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_stars_and_question_marks() {
        assert!(matches("GDLVL*.8xv", "GDLVL1.8xv"));
        assert!(matches("GDLVL*.8xv", "GDLVL.8xv"));
        assert!(matches("A?C", "ABC"));
        assert!(matches("*a*b*", "xxaxxbxx"));
        assert!(!matches("GDLVL*.8xv", "GDLVL1.8xp"));
        assert!(!matches("A?C", "AC"));
    }

    #[test]
    fn expands_patterns_in_folders() {
        let root = std::env::temp_dir().join(format!("ceagle-glob-{}", std::process::id()));
        fs::create_dir_all(root.join("bin/deep")).unwrap();
        for name in [
            "bin/GDASH.8xp",
            "bin/GDLVL1.8xv",
            "bin/GDLVL2.8xv",
            "bin/deep/GDLVL3.8xv",
            "bin/.GDLVL4.8xv",
        ] {
            fs::write(root.join(name), b"").unwrap();
        }
        assert_eq!(
            expand(&root, "bin/GDLVL*.8xv").unwrap(),
            ["bin/GDLVL1.8xv", "bin/GDLVL2.8xv"]
        );
        assert_eq!(
            expand(&root, "**/GDLVL*.8xv").unwrap(),
            ["bin/GDLVL1.8xv", "bin/GDLVL2.8xv", "bin/deep/GDLVL3.8xv"]
        );
        assert_eq!(expand(&root, "./bin/GDASH.8xp").unwrap(), ["bin/GDASH.8xp"]);
        assert!(expand(&root, "bin/MISSING.8xp").unwrap().is_empty());
        assert!(expand(&root, "nothing/*.8xp").unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
