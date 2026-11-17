//! Filesystem completion for the raw-mode `n` image loader.
//! Unlike a shell prompt, raw terminal input never goes through zsh completion.
use std::{fs, path::PathBuf};

#[derive(Debug)]
pub struct PathMatches {
    values: Vec<String>,
    selected: Option<usize>,
}
impl PathMatches {
    pub fn status(&self) -> String {
        let count = self.values.len();
        if count == 0 {
            return "No matching NIfTI files or directories".into();
        }
        if count == 1 {
            return "1 match".into();
        }
        let shown = self
            .values
            .iter()
            .take(4)
            .map(|s| {
                s.rsplit('/')
                    .filter(|part| !part.is_empty())
                    .next()
                    .unwrap_or(s)
            })
            .collect::<Vec<_>>()
            .join("  ·  ");
        format!("{count} matches · Tab cycle · Shift-Tab back · {shown}")
    }
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest);
    }
    PathBuf::from(path)
}

pub fn resolve(path: &str) -> PathBuf {
    expand_home(path)
}

fn candidates(prefix: &str) -> Vec<String> {
    let (dir, fragment) = match prefix.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name),
        None => (String::new(), prefix),
    };
    let base = if dir.is_empty() {
        PathBuf::from(".")
    } else {
        expand_home(&dir)
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(fragment) || (name.starts_with('.') && !fragment.starts_with('.')) {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        if !is_dir && !(name.ends_with(".nii.gz") || name.ends_with(".nii")) {
            continue;
        }
        values.push(format!("{}{}{}", dir, name, if is_dir { "/" } else { "" }));
    }
    values.sort_unstable();
    values
}

fn common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in values.iter().skip(1) {
        let len = prefix
            .chars()
            .zip(value.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = prefix.chars().take(len).collect();
    }
    prefix
}

/// First Tab extends the unambiguous prefix. Repeated Tab cycles matches.
/// Shift-Tab cycles in reverse. Completing a directory permits browsing inside.
pub fn advance(text: &mut String, state: &mut Option<PathMatches>, reverse: bool) {
    if state.is_none() {
        let values = candidates(text);
        if values.is_empty() {
            *state = Some(PathMatches {
                values,
                selected: None,
            });
            return;
        }
        if values.len() == 1 {
            *text = values[0].clone();
            // Directories should be browsable on the next Tab.
            *state = None;
            return;
        }
        let common = common_prefix(&values);
        if common.chars().count() > text.chars().count() {
            *text = common;
            *state = Some(PathMatches {
                values,
                selected: None,
            });
            return;
        }
        *state = Some(PathMatches {
            values,
            selected: None,
        });
        return;
    }
    let matches = state.as_mut().expect("checked above");
    if matches.values.is_empty() {
        return;
    }
    let next = match matches.selected {
        None => {
            if reverse {
                matches.values.len() - 1
            } else {
                0
            }
        }
        Some(i) => {
            if reverse {
                (i + matches.values.len() - 1) % matches.values.len()
            } else {
                (i + 1) % matches.values.len()
            }
        }
    };
    *text = matches.values[next].clone();
    matches.selected = Some(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn tabs_extend_prefix_cycle_and_follow_directories() {
        let p = std::env::temp_dir().join(format!(
            "niv-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        for s in ["brain_01.nii.gz", "brain_02.nii.gz", "ignore.txt"] {
            fs::write(p.join(s), b"").unwrap();
        }
        fs::create_dir(p.join("other")).unwrap();
        let mut input = format!("{}/br", p.display());
        let mut state = None;
        advance(&mut input, &mut state, false);
        assert!(input.ends_with("/brain_0"));
        advance(&mut input, &mut state, false);
        assert!(input.ends_with("/brain_01.nii.gz"));
        advance(&mut input, &mut state, false);
        assert!(input.ends_with("/brain_02.nii.gz"));
        advance(&mut input, &mut state, true);
        assert!(input.ends_with("/brain_01.nii.gz"));
        let mut input = format!("{}/ot", p.display());
        let mut state = None;
        advance(&mut input, &mut state, false);
        assert!(input.ends_with("/other/"));
        assert!(state.is_none());
        fs::remove_dir_all(&p).unwrap();
    }
}
