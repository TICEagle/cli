use crate::variable::{
    NAME_SIZE, TYPE_APPVAR, TYPE_IMAGE, TYPE_MATRIX, TYPE_PICTURE, TYPE_PROGRAM,
    TYPE_PROTECTED_PROGRAM, TYPE_REAL_LIST, TYPE_STRING,
};

const THETA: u8 = 0x5B;
const LIST_TOKEN: u8 = 0x5D;
const MATRIX_TOKEN: u8 = 0x5C;
const STRING_TOKEN: u8 = 0xAA;
const PICTURE_TOKEN: u8 = 0x60;
const IMAGE_TOKEN: u8 = 0x3C;
const BUILTIN_LISTS: u8 = 6;
const NUMBERED_VARIABLES: u8 = 10;
const MAX_LIST_NAME: usize = 5;

pub fn trim_name(kind: u8, field: &[u8]) -> &[u8] {
    let used = field
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |at| at + 1);
    if (numbered_token(kind).is_some() || kind == TYPE_REAL_LIST) && used < 2 {
        return &field[..2];
    }
    &field[..used]
}

fn numbered_token(kind: u8) -> Option<u8> {
    match kind {
        TYPE_MATRIX => Some(MATRIX_TOKEN),
        TYPE_STRING => Some(STRING_TOKEN),
        TYPE_PICTURE => Some(PICTURE_TOKEN),
        TYPE_IMAGE => Some(IMAGE_TOKEN),
        _ => None,
    }
}

fn valid_chars(name: &[u8], max_length: usize, lowercase: bool) -> bool {
    !name.is_empty()
        && name.len() <= max_length
        && name.iter().enumerate().all(|(index, &char)| {
            char.is_ascii_uppercase()
                || char == THETA
                || lowercase && char.is_ascii_lowercase()
                || index > 0 && char.is_ascii_digit()
        })
}

pub fn valid_name(kind: u8, name: &[u8]) -> bool {
    match kind {
        TYPE_PROGRAM | TYPE_PROTECTED_PROGRAM => valid_chars(name, NAME_SIZE, false),
        TYPE_APPVAR => valid_chars(name, NAME_SIZE, true),
        TYPE_REAL_LIST => match name {
            [LIST_TOKEN, number] => {
                *number < BUILTIN_LISTS || valid_chars(&name[1..], MAX_LIST_NAME, false)
            }
            [LIST_TOKEN, rest @ ..] => valid_chars(rest, MAX_LIST_NAME, false),
            _ => false,
        },
        _ => match (numbered_token(kind), name) {
            (Some(token), [first, number]) => *first == token && *number < NUMBERED_VARIABLES,
            _ => false,
        },
    }
}

fn readable(name: &[u8]) -> String {
    name.iter()
        .map(|&char| {
            if char == THETA {
                'θ'
            } else {
                char::from(char)
            }
        })
        .collect()
}

pub fn display_name(kind: u8, name: &[u8]) -> String {
    if !valid_name(kind, name) {
        return String::new();
    }
    let number = name.get(1).copied().unwrap_or_default();
    let digit = (u16::from(number) + 1) % u16::from(NUMBERED_VARIABLES);
    match kind {
        TYPE_PROGRAM | TYPE_PROTECTED_PROGRAM | TYPE_APPVAR => readable(name),
        TYPE_REAL_LIST if name.len() == 2 && number < BUILTIN_LISTS => format!("L{}", number + 1),
        TYPE_REAL_LIST => format!("∟{}", readable(&name[1..])),
        TYPE_MATRIX => format!("[{}]", char::from(b'A' + number)),
        TYPE_STRING => format!("Str{digit}"),
        TYPE_PICTURE => format!("Pic{digit}"),
        _ => format!("Image{digit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variable::{build, parse, sized};

    #[test]
    fn names_numbered_and_list_variables() {
        let list = build(
            TYPE_REAL_LIST,
            &[LIST_TOKEN, b'H', b'I'],
            &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(parse(&list).unwrap().display_name(), "∟HI");
        let builtin = build(
            TYPE_REAL_LIST,
            &[LIST_TOKEN, 0],
            &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(parse(&builtin).unwrap().display_name(), "L1");
        let string = build(TYPE_STRING, &[STRING_TOKEN, 9], &sized(b"A"));
        assert_eq!(parse(&string).unwrap().display_name(), "Str0");
    }
}
