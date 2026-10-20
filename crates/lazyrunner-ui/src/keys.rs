//! Key specs for injected input ("l l z", "Tab", "Ctrl+c"), so tests press the same keys a person does.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn parse(spec: &str) -> Result<Vec<KeyEvent>, String> {
    spec.split_whitespace().map(parse_one).collect()
}

fn parse_one(token: &str) -> Result<KeyEvent, String> {
    if let Some(rest) = token.strip_prefix("Ctrl+").or_else(|| token.strip_prefix("C-")) {
        let key = parse_one(rest)?;
        return Ok(KeyEvent::new(key.code, key.modifiers | KeyModifiers::CONTROL));
    }
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let modifiers = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        return Ok(KeyEvent::new(KeyCode::Char(c), modifiers));
    }
    let code = match token {
        "Tab" => KeyCode::Tab,
        "BackTab" => return Ok(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Space" => KeyCode::Char(' '),
        "Backspace" => KeyCode::Backspace,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        _ => return Err(format!("unknown key '{token}'")),
    };
    Ok(KeyEvent::new(code, KeyModifiers::NONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_letters_names_and_ctrl() {
        let keys = parse("l G Tab Ctrl+c PageUp").unwrap();
        assert_eq!(keys[0], KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(keys[1], KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(keys[2].code, KeyCode::Tab);
        assert_eq!(keys[3], KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(keys[4].code, KeyCode::PageUp);
    }

    #[test]
    fn rejects_what_it_doesnt_know() {
        assert_eq!(parse("l Hyper").unwrap_err(), "unknown key 'Hyper'");
    }
}
