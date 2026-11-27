use std::fmt;

use crate::name::{display_name, trim_name, valid_name};

const SIGNATURE: &[u8] = b"**TI83F*\x1a\x0a";
const HEADER_SIZE: usize = 55;
const SECTION_LENGTH_AT: usize = 53;
const CHECKSUM_SIZE: usize = 2;
pub const NAME_SIZE: usize = 8;
const SHORT_ENTRY: usize = 11;
const LONG_ENTRY: usize = 13;
const ARCHIVED_FLAG: u8 = 0x80;
const MAX_LIST_LENGTH: usize = 999;
const MAX_MATRIX_SIDE: usize = 99;
const REAL_NUMBER_BYTES: usize = 9;
const ASM_PROLOGUE: &[u8] = &[0xEF, 0x7B];
const TWO_BYTE_TOKEN_FIRST: &[u8] = &[
    0x5C, 0x5D, 0x5E, 0x60, 0x61, 0x62, 0x63, 0x7E, 0xAA, 0xBB, 0xEF,
];
const LIBRARY_MARKER: u8 = 0xC0;
const OPTIONAL_LIBRARY_MARKER: u8 = 0xC1;

pub const MAX_FILE_SIZE: usize = HEADER_SIZE + u16::MAX as usize + CHECKSUM_SIZE;
pub const TYPE_REAL_LIST: u8 = 0x01;
pub const TYPE_MATRIX: u8 = 0x02;
pub const TYPE_STRING: u8 = 0x04;
pub const TYPE_PROGRAM: u8 = 0x05;
pub const TYPE_PROTECTED_PROGRAM: u8 = 0x06;
pub const TYPE_PICTURE: u8 = 0x07;
pub const TYPE_APPVAR: u8 = 0x15;
pub const TYPE_IMAGE: u8 = 0x1A;

#[derive(Debug, PartialEq, Eq)]
pub enum VariableError {
    TooShort,
    Signature,
    Section,
    Checksum,
    EntryHeader,
    Truncated,
    Lengths,
    ManyVariables,
    Unsupported(u8),
    Name(u8),
    Data(u8),
}

impl fmt::Display for VariableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VariableError::TooShort => {
                write!(formatter, "the file is shorter than a TI variable header")
            }
            VariableError::Signature => write!(
                formatter,
                "the file does not start with the **TI83F* signature"
            ),
            VariableError::Section => write!(
                formatter,
                "the data section length does not match the file size"
            ),
            VariableError::Checksum => {
                write!(formatter, "the checksum does not match the data section")
            }
            VariableError::EntryHeader => {
                write!(formatter, "the variable header length is not 11 or 13")
            }
            VariableError::Truncated => {
                write!(formatter, "the variable runs past the end of the file")
            }
            VariableError::Lengths => {
                write!(formatter, "the two variable length fields disagree")
            }
            VariableError::ManyVariables => {
                write!(formatter, "the file holds more than one variable")
            }
            VariableError::Unsupported(kind) => {
                write!(
                    formatter,
                    "{} variables are not supported",
                    type_name(*kind)
                )
            }
            VariableError::Name(kind) => {
                write!(
                    formatter,
                    "the name is not a valid {} name",
                    type_name(*kind)
                )
            }
            VariableError::Data(kind) => write!(
                formatter,
                "the {} length does not match its data",
                type_name(*kind)
            ),
        }
    }
}

impl std::error::Error for VariableError {}

#[derive(Debug, PartialEq, Eq)]
pub struct Variable<'a> {
    pub kind: u8,
    pub name: &'a [u8],
    pub archived: bool,
    pub data: &'a [u8],
}

pub fn type_name(kind: u8) -> String {
    match kind {
        TYPE_REAL_LIST => "list".into(),
        TYPE_MATRIX => "matrix".into(),
        TYPE_STRING => "string".into(),
        TYPE_PROGRAM => "program".into(),
        TYPE_PROTECTED_PROGRAM => "protected program".into(),
        TYPE_PICTURE => "picture".into(),
        TYPE_APPVAR => "appvar".into(),
        TYPE_IMAGE => "image".into(),
        other => format!("type 0x{other:02X}"),
    }
}

pub fn extension(kind: u8) -> Option<&'static str> {
    match kind {
        TYPE_REAL_LIST => Some(".8xl"),
        TYPE_MATRIX => Some(".8xm"),
        TYPE_STRING => Some(".8xs"),
        TYPE_PROGRAM | TYPE_PROTECTED_PROGRAM => Some(".8xp"),
        TYPE_PICTURE => Some(".8ci"),
        TYPE_APPVAR => Some(".8xv"),
        TYPE_IMAGE => Some(".8ca"),
        _ => None,
    }
}

fn word(bytes: &[u8], at: usize) -> Option<usize> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    Some(usize::from(u16::from_le_bytes([pair[0], pair[1]])))
}

fn checksum(section: &[u8]) -> u16 {
    section
        .iter()
        .fold(0u16, |sum, &value| sum.wrapping_add(u16::from(value)))
}

pub fn parse(file: &[u8]) -> Result<Variable<'_>, VariableError> {
    if file.len() < HEADER_SIZE + CHECKSUM_SIZE {
        return Err(VariableError::TooShort);
    }
    if !file.starts_with(SIGNATURE) {
        return Err(VariableError::Signature);
    }
    let length = word(file, SECTION_LENGTH_AT).ok_or(VariableError::TooShort)?;
    if HEADER_SIZE + length + CHECKSUM_SIZE != file.len() {
        return Err(VariableError::Section);
    }
    let section = &file[HEADER_SIZE..HEADER_SIZE + length];
    let stored = word(file, HEADER_SIZE + length).ok_or(VariableError::Section)?;
    if usize::from(checksum(section)) != stored {
        return Err(VariableError::Checksum);
    }
    parse_entry(section)
}

fn parse_entry(section: &[u8]) -> Result<Variable<'_>, VariableError> {
    let header_length = word(section, 0).ok_or(VariableError::Truncated)?;
    if header_length != SHORT_ENTRY && header_length != LONG_ENTRY {
        return Err(VariableError::EntryHeader);
    }
    let start = 2 + header_length + 2;
    let header = section
        .get(2..2 + header_length)
        .ok_or(VariableError::Truncated)?;
    let size = word(header, 0).ok_or(VariableError::Truncated)?;
    let second = word(section, 2 + header_length).ok_or(VariableError::Truncated)?;
    if size != second {
        return Err(VariableError::Lengths);
    }
    let end = start + size;
    if end > section.len() {
        return Err(VariableError::Truncated);
    }
    if end < section.len() {
        return Err(VariableError::ManyVariables);
    }
    let kind = header[2];
    let variable = Variable {
        kind,
        name: trim_name(kind, &header[3..3 + NAME_SIZE]),
        archived: header_length == LONG_ENTRY && header[12] & ARCHIVED_FLAG != 0,
        data: &section[start..],
    };
    variable.check()?;
    Ok(variable)
}

fn check_data(kind: u8, data: &[u8]) -> Result<(), VariableError> {
    let count = word(data, 0).ok_or(VariableError::Data(kind))?;
    let valid = match kind {
        TYPE_REAL_LIST => {
            (1..=MAX_LIST_LENGTH).contains(&count) && data.len() == 2 + count * REAL_NUMBER_BYTES
        }
        TYPE_MATRIX => {
            let (columns, rows) = (usize::from(data[0]), usize::from(data[1]));
            (1..=MAX_MATRIX_SIDE).contains(&columns)
                && (1..=MAX_MATRIX_SIDE).contains(&rows)
                && data.len() == 2 + columns * rows * REAL_NUMBER_BYTES
        }
        _ => count == data.len() - 2,
    };
    if valid {
        Ok(())
    } else {
        Err(VariableError::Data(kind))
    }
}

impl Variable<'_> {
    fn check(&self) -> Result<(), VariableError> {
        if extension(self.kind).is_none() {
            return Err(VariableError::Unsupported(self.kind));
        }
        if !valid_name(self.kind, self.name) {
            return Err(VariableError::Name(self.kind));
        }
        check_data(self.kind, self.data)
    }

    pub fn display_name(&self) -> String {
        display_name(self.kind, self.name)
    }

    fn body(&self) -> Option<&[u8]> {
        if self.kind == TYPE_REAL_LIST || self.kind == TYPE_MATRIX {
            return None;
        }
        (word(self.data, 0)? == self.data.len() - 2).then(|| &self.data[2..])
    }

    pub fn is_asm(&self) -> bool {
        self.is_program()
            && self
                .body()
                .is_some_and(|body| body.starts_with(ASM_PROLOGUE))
    }

    pub fn is_basic(&self) -> bool {
        self.is_program() && !self.is_asm() && self.body().is_some_and(readable_tokens)
    }

    fn is_program(&self) -> bool {
        self.kind == TYPE_PROGRAM || self.kind == TYPE_PROTECTED_PROGRAM
    }

    pub fn library_version(&self) -> Option<u8> {
        match (self.kind, self.body()?) {
            (TYPE_APPVAR, [LIBRARY_MARKER, OPTIONAL_LIBRARY_MARKER, version, ..]) => Some(*version),
            _ => None,
        }
    }
}

fn readable_tokens(body: &[u8]) -> bool {
    let mut index = 0;
    while let Some(&token) = body.get(index) {
        if token == 0 {
            return false;
        }
        if TWO_BYTE_TOKEN_FIRST.contains(&token) {
            if index + 1 >= body.len() {
                return false;
            }
            index += 1;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
pub fn build(kind: u8, name: &[u8], data: &[u8]) -> Vec<u8> {
    let section_length = 2 + LONG_ENTRY + 2 + data.len();
    let mut file = SIGNATURE.to_vec();
    file.push(0);
    file.extend_from_slice(&[0; 42]);
    file.extend_from_slice(&(section_length as u16).to_le_bytes());
    file.extend_from_slice(&(LONG_ENTRY as u16).to_le_bytes());
    file.extend_from_slice(&(data.len() as u16).to_le_bytes());
    file.push(kind);
    let mut field = name.to_vec();
    field.resize(NAME_SIZE, 0);
    file.extend_from_slice(&field);
    file.extend_from_slice(&[0, ARCHIVED_FLAG]);
    file.extend_from_slice(&(data.len() as u16).to_le_bytes());
    file.extend_from_slice(data);
    let sum = checksum(&file[HEADER_SIZE..]);
    file.extend_from_slice(&sum.to_le_bytes());
    file
}

#[cfg(test)]
pub fn sized(body: &[u8]) -> Vec<u8> {
    let mut data = (body.len() as u16).to_le_bytes().to_vec();
    data.extend_from_slice(body);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = "../tivar/testdata";

    fn basic_program() -> Vec<u8> {
        build(TYPE_PROGRAM, b"HELLO", &sized(&[0xDE, 0x2A, 0x48, 0x2A]))
    }

    #[test]
    fn parses_a_basic_program() {
        let file = basic_program();
        let variable = parse(&file).unwrap();
        assert_eq!(variable.display_name(), "HELLO");
        assert!(variable.is_basic());
        assert!(variable.archived);
    }

    #[test]
    fn reads_toolchain_fixtures_when_present() {
        let Ok(program) = std::fs::read(format!("{FIXTURES}/CEFIX.8xp")) else {
            return;
        };
        assert!(parse(&program).unwrap().is_asm());
        let library = std::fs::read(format!("{FIXTURES}/FIXLIB.8xv")).unwrap();
        assert!(parse(&library).unwrap().library_version().is_some());
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let mut file = basic_program();
        let last = file.len() - 1;
        file[last] ^= 1;
        assert_eq!(parse(&file), Err(VariableError::Checksum));
    }

    #[test]
    fn rejects_every_truncation() {
        let file = basic_program();
        for length in 0..file.len() {
            assert!(parse(&file[..length]).is_err());
        }
    }

    #[test]
    fn rejects_lengths_past_the_end() {
        let mut file = basic_program();
        file[HEADER_SIZE + 2] = 0xFF;
        file[HEADER_SIZE + 2 + LONG_ENTRY] = 0xFF;
        let length = file.len();
        let sum = checksum(&file[HEADER_SIZE..length - 2]);
        file[length - 2..].copy_from_slice(&sum.to_le_bytes());
        assert_eq!(parse(&file), Err(VariableError::Truncated));
    }

    #[test]
    fn rejects_bad_names() {
        let file = build(TYPE_PROGRAM, b"1ABC", &sized(&[0x2A]));
        assert_eq!(parse(&file), Err(VariableError::Name(TYPE_PROGRAM)));
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let template = basic_program();
        for _ in 0..20_000 {
            let mut file = if next() % 2 == 0 {
                template.clone()
            } else {
                let mut header = SIGNATURE.to_vec();
                header.resize((next() % 200) as usize, 0);
                header
            };
            for _ in 0..next() % 8 {
                if !file.is_empty() {
                    let at = (next() as usize) % file.len();
                    file[at] = next() as u8;
                }
            }
            if next() % 2 == 0 && file.len() >= HEADER_SIZE + CHECKSUM_SIZE {
                let end = file.len() - CHECKSUM_SIZE;
                let sum = checksum(&file[HEADER_SIZE..end]);
                file[end..].copy_from_slice(&sum.to_le_bytes());
            }
            if let Ok(variable) = parse(&file) {
                let _ = (
                    variable.display_name(),
                    variable.is_basic(),
                    variable.library_version(),
                );
            }
        }
    }
}
