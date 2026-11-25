//! Turns key presses into the bytes a terminal program expects.

use winit::keyboard::{Key, NamedKey};

/// Modifier keys held during a key press.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// The name of a key as the configuration spells it (`"V"`, `"PageUp"`, `"Up"`, ...), or
/// `None` for keys that have no name.
pub fn key_name(key: &Key) -> Option<String> {
    let name = match key {
        Key::Character(s) => return Some(s.to_ascii_uppercase()),
        Key::Named(named) => match named {
            NamedKey::Enter => "Enter",
            NamedKey::Tab => "Tab",
            NamedKey::Space => "Space",
            NamedKey::Escape => "Escape",
            NamedKey::Backspace => "Backspace",
            NamedKey::Delete => "Delete",
            NamedKey::Insert => "Insert",
            NamedKey::Home => "Home",
            NamedKey::End => "End",
            NamedKey::PageUp => "PageUp",
            NamedKey::PageDown => "PageDown",
            NamedKey::ArrowUp => "Up",
            NamedKey::ArrowDown => "Down",
            NamedKey::ArrowLeft => "Left",
            NamedKey::ArrowRight => "Right",
            NamedKey::F1 => "F1",
            NamedKey::F2 => "F2",
            NamedKey::F3 => "F3",
            NamedKey::F4 => "F4",
            NamedKey::F5 => "F5",
            NamedKey::F6 => "F6",
            NamedKey::F7 => "F7",
            NamedKey::F8 => "F8",
            NamedKey::F9 => "F9",
            NamedKey::F10 => "F10",
            NamedKey::F11 => "F11",
            NamedKey::F12 => "F12",
            _ => return None,
        },
        _ => return None,
    };
    Some(name.to_string())
}

/// The bytes for a key press, or `None` if the key sends nothing.
/// `text` is what the keyboard layout produced for the press, if anything.
pub fn key_bytes(key: &Key, text: Option<&str>, mods: Mods) -> Option<Vec<u8>> {
    let mut out = match key {
        Key::Named(named) => named_key_bytes(*named, mods)?.to_vec(),
        Key::Character(s) if mods.ctrl => control_bytes(s)?,
        _ => match text {
            Some(t) => t.as_bytes().to_vec(),
            None => match key {
                Key::Character(s) => s.as_bytes().to_vec(),
                _ => return None,
            },
        },
    };
    if mods.alt {
        out.insert(0, 0x1b); // Alt sends ESC first
    }
    Some(out)
}

fn named_key_bytes(key: NamedKey, mods: Mods) -> Option<&'static [u8]> {
    Some(match key {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => b"\x7f",
        NamedKey::Tab if mods.shift => b"\x1b[Z",
        NamedKey::Tab => b"\t",
        NamedKey::Escape => b"\x1b",
        NamedKey::Space => b" ",
        NamedKey::ArrowUp => b"\x1b[A",
        NamedKey::ArrowDown => b"\x1b[B",
        NamedKey::ArrowRight => b"\x1b[C",
        NamedKey::ArrowLeft => b"\x1b[D",
        NamedKey::Home => b"\x1b[H",
        NamedKey::End => b"\x1b[F",
        NamedKey::Insert => b"\x1b[2~",
        NamedKey::Delete => b"\x1b[3~",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        NamedKey::F1 => b"\x1bOP",
        NamedKey::F2 => b"\x1bOQ",
        NamedKey::F3 => b"\x1bOR",
        NamedKey::F4 => b"\x1bOS",
        NamedKey::F5 => b"\x1b[15~",
        NamedKey::F6 => b"\x1b[17~",
        NamedKey::F7 => b"\x1b[18~",
        NamedKey::F8 => b"\x1b[19~",
        NamedKey::F9 => b"\x1b[20~",
        NamedKey::F10 => b"\x1b[21~",
        NamedKey::F11 => b"\x1b[23~",
        NamedKey::F12 => b"\x1b[24~",
        _ => return None,
    })
}

/// The bytes for cursor keys when the program has asked for application cursor keys
/// (DECCKM, mode 1). Programs such as vim and less expect `ESC O x` for these.
pub fn application_cursor_bytes(key: &Key) -> Option<&'static [u8]> {
    Some(match key {
        Key::Named(NamedKey::ArrowUp) => b"\x1bOA",
        Key::Named(NamedKey::ArrowDown) => b"\x1bOB",
        Key::Named(NamedKey::ArrowRight) => b"\x1bOC",
        Key::Named(NamedKey::ArrowLeft) => b"\x1bOD",
        Key::Named(NamedKey::Home) => b"\x1bOH",
        Key::Named(NamedKey::End) => b"\x1bOF",
        _ => return None,
    })
}

/// Ctrl plus a key: the matching control byte (Ctrl-A = 0x01, Ctrl-C = 0x03, ...).
fn control_bytes(s: &str) -> Option<Vec<u8>> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let byte = match c {
        'a'..='z' | 'A'..='Z' => c.to_ascii_lowercase() as u8 & 0x1f,
        ' ' | '@' => 0x00,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        _ => return None,
    };
    Some(vec![byte])
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }

    #[test]
    fn enter_and_arrows() {
        assert_eq!(key_bytes(&Key::Named(NamedKey::Enter), None, Mods::default()), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&Key::Named(NamedKey::ArrowUp), None, Mods::default()), Some(b"\x1b[A".to_vec()));
    }

    #[test]
    fn shift_tab_is_backtab() {
        let m = Mods { shift: true, ..Mods::default() };
        assert_eq!(key_bytes(&Key::Named(NamedKey::Tab), None, m), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn ctrl_letters_are_control_bytes() {
        let m = Mods { ctrl: true, ..Mods::default() };
        assert_eq!(key_bytes(&ch("c"), Some("c"), m), Some(vec![0x03]));
        assert_eq!(key_bytes(&ch("A"), None, m), Some(vec![0x01]));
        assert_eq!(key_bytes(&ch("]"), None, m), Some(vec![0x1d]));
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(key_bytes(&ch("x"), Some("x"), Mods::default()), Some(b"x".to_vec()));
        assert_eq!(key_bytes(&ch("é"), Some("é"), Mods::default()), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn alt_prefixes_escape() {
        let m = Mods { alt: true, ..Mods::default() };
        assert_eq!(key_bytes(&ch("x"), Some("x"), m), Some(b"\x1bx".to_vec()));
    }

    #[test]
    fn key_names_match_the_config_spelling() {
        assert_eq!(key_name(&ch("v")).as_deref(), Some("V"));
        assert_eq!(key_name(&Key::Named(NamedKey::PageUp)).as_deref(), Some("PageUp"));
        assert_eq!(key_name(&Key::Named(NamedKey::ArrowUp)).as_deref(), Some("Up"));
        assert_eq!(key_name(&Key::Named(NamedKey::Shift)), None);
    }

    #[test]
    fn unknown_named_keys_send_nothing() {
        assert_eq!(key_bytes(&Key::Named(NamedKey::Shift), None, Mods::default()), None);
    }
}
