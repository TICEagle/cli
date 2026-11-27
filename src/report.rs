use crate::{api::DryRunDependency, release::LocalFile};

const KIB: usize = 1024;
const CAPABILITY_LABELS: &[(&str, &str)] = &[
    ("flash_write", "Writes to flash memory"),
    ("delete_vars", "Deletes variables"),
    ("ram_reset", "Can clear RAM"),
    ("usb", "Uses USB"),
    ("network", "Uses the network"),
    ("inline_asm", "Runs assembly code from TI-BASIC"),
    ("link", "Sends or receives over the link port"),
];

fn size(bytes: usize) -> String {
    if bytes < KIB {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    }
}

pub fn file_table(files: &[LocalFile], role_of: impl Fn(&LocalFile) -> String) -> String {
    let mut rows = vec![[
        "FILE".to_string(),
        "VARIABLE".into(),
        "DETECTED".into(),
        "SIZE".into(),
        "ROLE".into(),
    ]];
    for file in files {
        rows.push([
            file.path.clone(),
            file.variable_name.clone(),
            file.detected.clone(),
            size(file.content.len()),
            role_of(file),
        ]);
    }
    let mut widths = [0; 5];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut table = String::new();
    for row in &rows {
        let mut line = String::from(" ");
        for (width, cell) in widths.iter().zip(row) {
            line.push(' ');
            line.push_str(cell);
            line.push_str(&" ".repeat(width - cell.chars().count() + 1));
        }
        table.push_str(line.trim_end());
        table.push('\n');
    }
    table
}

pub fn problems(list: &[String]) -> String {
    let noun = if list.len() == 1 {
        "problem"
    } else {
        "problems"
    };
    let mut text = format!("{} {noun} to fix before publishing:\n", list.len());
    for problem in list {
        text.push_str(&format!("  - {problem}\n"));
    }
    text
}

pub fn dependencies(list: &[DryRunDependency]) -> Option<String> {
    let described: Vec<String> = list
        .iter()
        .map(|dependency| {
            let minimum = match (&dependency.min_version, dependency.min_libload_version) {
                (Some(version), _) => format!(" >={version}"),
                (None, Some(version)) => format!(" version {version} or newer"),
                (None, None) => String::new(),
            };
            let found = if dependency.auto {
                ", found in the program"
            } else {
                ""
            };
            format!("{}{minimum}{found}", dependency.slug)
        })
        .collect();
    (!described.is_empty()).then(|| format!("Dependencies: {}", described.join("; ")))
}

pub fn capabilities(names: &[String]) -> String {
    if names.is_empty() {
        return "Capabilities: none".into();
    }
    let labels: Vec<&str> = names
        .iter()
        .map(|name| {
            CAPABILITY_LABELS
                .iter()
                .find(|(known, _)| known == name)
                .map_or(name.as_str(), |(_, label)| label)
        })
        .collect();
    format!("Capabilities: {}", labels.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_capabilities_and_keeps_unknown_names() {
        let names = vec![
            "usb".to_string(),
            "delete_vars".into(),
            "future_thing".into(),
        ];
        assert_eq!(
            capabilities(&names),
            "Capabilities: Uses USB, Deletes variables, future_thing"
        );
        assert_eq!(capabilities(&[]), "Capabilities: none");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1536), "1.5 KiB");
    }
}
