//! Fast, in-TUI NIfTI file picker. Shell completion is not active in raw mode.
//! Match any substring of a filename; keep directory navigation explicit so a
//! large research tree is never scanned recursively on every keystroke.
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct PathMatch {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct PathMatches {
    pub values: Vec<PathMatch>,
    pub selected: Option<usize>,
    pub directory: String,
}

impl PathMatches {
    pub fn status(&self) -> String {
        let count = self.values.len();
        if count == 0 {
            "No matching NIfTI files or directories".into()
        } else {
            format!("{count} matches · ↑↓ choose · Tab complete/cycle · Space mark · Enter load")
        }
    }
    pub fn selected_index(&self) -> usize {
        self.selected.unwrap_or(0)
    }
    pub fn selected(&self) -> Option<&PathMatch> {
        self.values.get(self.selected_index())
    }
    pub fn move_by(&mut self, delta: isize) {
        if self.values.is_empty() {
            return;
        }
        let current = self.selected_index() as isize;
        self.selected = Some((current + delta).rem_euclid(self.values.len() as isize) as usize);
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

/// Query zoxide only on explicit Enter, never during filename filtering.
/// No shell evaluation and no process-wide current-directory changes.
pub fn zoxide_directory(query: &str) -> Result<String, String> {
    let keywords: Vec<_> = query.split_whitespace().collect();
    if keywords.is_empty() {
        return Err("Type at least one directory keyword".into());
    }
    let output = std::process::Command::new("zoxide")
        .args(["query", "--"])
        .args(&keywords)
        .output()
        .map_err(|e| format!("Cannot run zoxide: {e}"))?;
    if !output.status.success() {
        return Err("No matching zoxide directory (or zoxide database unavailable)".into());
    }
    directory_filter(&String::from_utf8_lossy(&output.stdout))
}

fn directory_filter(result: &str) -> Result<String, String> {
    let dir = PathBuf::from(result.trim_end_matches(['\r', '\n']));
    if !dir.is_absolute() || !dir.is_dir() {
        return Err("Zoxide did not return an existing absolute directory".into());
    }
    let mut text = dir.to_string_lossy().into_owned();
    if !text.ends_with('/') {
        text.push('/');
    }
    Ok(text)
}

fn dir_and_fragment(input: &str) -> (String, &str) {
    match input.rsplit_once('/') {
        Some((dir, frag)) => (format!("{dir}/"), frag),
        None => (String::new(), input),
    }
}

fn wild_match(pattern: &str, name: &str) -> bool {
    // A simple case-insensitive glob for the *current filename*; unlike a
    // recursive glob it is bounded by the number of entries in one directory.
    let pat = pattern.as_bytes();
    let word = name.as_bytes();
    let (mut p, mut w, mut star, mut resume) = (0, 0, None, 0);
    while w < word.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p].eq_ignore_ascii_case(&word[w])) {
            p += 1;
            w += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = Some(p);
            p += 1;
            resume = w;
        } else if let Some(s) = star {
            p = s + 1;
            resume += 1;
            w = resume;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

pub fn search(input: &str) -> PathMatches {
    let (dir, fragment) = dir_and_fragment(input);
    let base = if dir.is_empty() {
        PathBuf::from(".")
    } else {
        expand_home(&dir)
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return PathMatches {
            values: Vec::new(),
            selected: None,
            directory: dir,
        };
    };
    let needle = fragment.to_lowercase();
    let wildcard = fragment.contains('*') || fragment.contains('?');
    let mut values = Vec::new();
    for entry in entries.flatten() {
        let filename = entry.file_name().to_string_lossy().into_owned();
        if filename.starts_with('.') && !fragment.starts_with('.') {
            continue;
        }
        let is_dir = entry.path().is_dir();
        let lower = filename.to_lowercase();
        if !is_dir && !(lower.ends_with(".nii") || lower.ends_with(".nii.gz")) {
            continue;
        }
        let matches = if wildcard {
            wild_match(fragment, &filename)
        } else {
            lower.contains(&needle)
        };
        if matches {
            values.push(PathMatch {
                path: format!("{dir}{filename}{}", if is_dir { "/" } else { "" }),
                is_dir,
            });
        }
    }
    // Directories first, then alphabetical filenames. Stable order across Tab presses.
    values.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    PathMatches {
        values,
        selected: None,
        directory: dir,
    }
}

fn common_prefix(values: &[PathMatch]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut prefix = first.path.clone();
    for item in values.iter().skip(1) {
        let len = prefix
            .chars()
            .zip(item.path.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = prefix.chars().take(len).collect();
    }
    prefix
}

/// Tab extends a true common *prefix* if one exists, otherwise cycles the
/// visible selection. A unique match completes directly. Shift-Tab reverses.
pub fn advance(text: &mut String, matches: &mut PathMatches, reverse: bool) {
    if matches.values.is_empty() {
        return;
    }
    if matches.values.len() == 1 {
        *text = matches.values[0].path.clone();
        *matches = search(text); // Tab into directories automatically
        return;
    }
    if matches.selected.is_none() && !reverse {
        // Substring search also matches e.g. posterior-Brain when the user typed 'br'.
        // For Tab extension, prefer filenames that actually BEGIN with the fragment.
        // Keep all substring hits for normal arrow/Tab selection when no extension exists.
        let (_, fragment) = dir_and_fragment(text);
        let needle = fragment.to_lowercase();
        let prefix_candidates: Vec<PathMatch> = matches
            .values
            .iter()
            .filter(|item| {
                let name = item
                    .path
                    .strip_prefix(matches.directory.as_str())
                    .unwrap_or(item.path.as_str());
                name.to_lowercase().starts_with(&needle)
            })
            .cloned()
            .collect();
        let prefix = common_prefix(&prefix_candidates);
        if prefix.to_lowercase().starts_with(&text.to_lowercase()) && prefix.len() > text.len() {
            *text = prefix;
            *matches = search(text);
            return;
        }
        matches.selected = Some(0);
    } else {
        matches.move_by(if reverse { -1 } else { 1 });
    }
}

pub fn is_nifti_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    (name.ends_with(".nii") || name.ends_with(".nii.gz")) && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn test_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "niv-path-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        for name in [
            "brain_anterior_01.nii.gz",
            "brain_anterior_02.nii.gz",
            "posterior-Brain.nii",
            "ignore.txt",
        ] {
            fs::write(path.join(name), b"").unwrap();
        }
        fs::create_dir(path.join("groupA")).unwrap();
        path
    }
    #[test]
    fn zoxide_result_must_be_an_existing_absolute_directory() {
        let root = test_directory();
        let filter = directory_filter(&format!("{}\n", root.display())).unwrap();
        assert!(filter.ends_with('/'));
        assert!(search(&filter)
            .values
            .iter()
            .any(|m| m.path.ends_with("posterior-Brain.nii")));
        assert!(directory_filter("relative/path").is_err());
        assert!(directory_filter("").is_err());
        assert!(zoxide_directory("  ").is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn substring_anywhere_is_case_insensitive_and_directories_are_browsable() {
        let root = test_directory();
        let list = search(&format!("{}/BRaIn", root.display()));
        assert_eq!(list.values.len(), 3);
        assert!(list
            .values
            .iter()
            .any(|m| m.path.ends_with("posterior-Brain.nii")));
        let folders = search(&format!("{}/gro", root.display()));
        assert!(folders.values[0].is_dir);
        let mut text = format!("{}/gro", root.display());
        let mut state = folders;
        advance(&mut text, &mut state, false);
        assert!(text.ends_with("/groupA/"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tab_extends_shared_prefix_and_cycles_selections_without_losing_filter() {
        let root = test_directory();
        let mut text = format!("{}/br", root.display());
        let mut state = search(&text);
        advance(&mut text, &mut state, false);
        assert!(text.ends_with("/brain_anterior_0"));
        advance(&mut text, &mut state, false);
        assert_eq!(state.selected_index(), 0);
        advance(&mut text, &mut state, false);
        assert_eq!(state.selected_index(), 1);
        assert!(text.ends_with("/brain_anterior_0"));
        advance(&mut text, &mut state, true);
        assert_eq!(state.selected_index(), 0);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tab_completion_prefers_prefix_even_with_uppercase_filter() {
        let root = test_directory();
        let mut text = format!("{}/BR", root.display());
        let mut state = search(&text);
        assert_eq!(state.values.len(), 3); // two prefix hits and one substring hit
        advance(&mut text, &mut state, false);
        assert!(text.ends_with("/brain_anterior_0"));
        assert_eq!(state.values.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tab_still_cycles_when_only_substring_matches_exist() {
        let root = test_directory();
        let mut text = format!("{}/terior", root.display());
        let mut state = search(&text);
        assert_eq!(state.values.len(), 3);
        advance(&mut text, &mut state, false);
        assert_eq!(state.selected, Some(0));
        assert!(text.ends_with("/terior"));
        advance(&mut text, &mut state, false);
        assert_eq!(state.selected, Some(1));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn wildcard_in_picker_filters_same_directory() {
        let root = test_directory();
        assert_eq!(
            search(&format!("{}/*anterior*", root.display()))
                .values
                .len(),
            2
        );
        assert!(wild_match("*posterior*", "posterior-Brain.nii"));
        fs::remove_dir_all(root).unwrap();
    }
}
