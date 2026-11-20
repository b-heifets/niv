//! Optional, context-specific key aliases. Text entry and global emergency
//! quit are deliberately outside this remapping layer.
use crate::app::App;
use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keymap {
    viewer: BTreeMap<String, String>,
    layers: BTreeMap<String, String>,
    montage: BTreeMap<String, String>,
    projection: BTreeMap<String, String>,
}
fn parse_key(text: &str) -> Result<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::NONE;
    let mut name = text;
    while let Some((head, rest)) = name.split_once('+') {
        let modifier = match head.to_ascii_lowercase().as_str() {
            "ctrl" => KeyModifiers::CONTROL,
            "shift" => KeyModifiers::SHIFT,
            "alt" => KeyModifiers::ALT,
            "cmd" | "super" => KeyModifiers::SUPER,
            _ => break,
        };
        mods |= modifier;
        name = rest;
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "enter" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "esc" => KeyCode::Esc,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        _ => {
            let mut chars = name.chars();
            let Some(c) = chars.next() else {
                bail!("Empty key")
            };
            if chars.next().is_some() {
                bail!("Unknown key {text}");
            }
            KeyCode::Char(c)
        }
    };
    Ok((code, mods))
}
fn emergency(code: KeyCode, m: KeyModifiers) -> bool {
    code == KeyCode::Char('\x03')
        || m.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'C'))
}
fn normalized(code: KeyCode, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if let KeyCode::Char(c) = code {
        if c.is_ascii_alphabetic() && (c.is_ascii_uppercase() || mods.contains(KeyModifiers::SHIFT))
        {
            return (
                KeyCode::Char(c.to_ascii_uppercase()),
                mods & !KeyModifiers::SHIFT,
            );
        }
    }
    if code == KeyCode::BackTab {
        return (KeyCode::Tab, mods | KeyModifiers::SHIFT);
    }
    (code, mods)
}
impl Keymap {
    pub fn load() -> Result<Self> {
        let explicit = std::env::var_os("NIV_KEYS").filter(|v| !v.is_empty());
        let path = explicit
            .clone()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME").map(|p| PathBuf::from(p).join("niv/keys.toml"))
            })
            .or_else(|| {
                std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/niv/keys.toml"))
            });
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && explicit.is_none() => {
                return Ok(Self::default())
            }
            Err(e) => return Err(e).with_context(|| format!("Read keymap {}", path.display())),
        };
        let map: Self =
            toml::from_str(&text).with_context(|| format!("Parse keymap {}", path.display()))?;
        for table in [&map.viewer, &map.layers, &map.montage, &map.projection] {
            for (a, b) in table {
                let (c, m) = parse_key(a)?;
                let (d, n) = parse_key(b)?;
                if emergency(c, m) || emergency(d, n) || c == KeyCode::Esc || d == KeyCode::Esc {
                    bail!("Ctrl+C and Esc cannot be remapped");
                }
            }
        }
        Ok(map)
    }
    pub fn remap(&self, app: &App, key: KeyEvent) -> KeyEvent {
        if app.edit.is_some()
            || app.popup.is_some()
            || emergency(key.code, key.modifiers)
            || key.code == KeyCode::Esc
        {
            return key;
        }
        let table = if app.show_layer_list && !app.fullscreen {
            &self.layers
        } else if app.montage.enabled {
            &self.montage
        } else if app.projection != crate::app::ProjectionMode::Slice {
            &self.projection
        } else {
            &self.viewer
        };
        for (a, b) in table {
            if let Ok((c, m)) = parse_key(a) {
                if normalized(c, m) == normalized(key.code, key.modifiers) {
                    if let Ok((code, modifiers)) = parse_key(b) {
                        return KeyEvent {
                            code,
                            modifiers,
                            ..key
                        };
                    }
                }
            }
        }
        key
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_names_and_shift() {
        assert_eq!(
            parse_key("Shift+Up").unwrap(),
            (KeyCode::Up, KeyModifiers::SHIFT)
        );
        assert_eq!(
            normalized(KeyCode::Char('f'), KeyModifiers::SHIFT),
            normalized(KeyCode::Char('F'), KeyModifiers::NONE)
        );
    }
}
