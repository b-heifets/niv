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

/// Ranked directory entries returned by zoxide. Paths (including spaces) are
/// preserved separately from the displayed score.
#[derive(Debug, Clone)]
pub struct RankedDirectory {
    pub score: f64,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreviewEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DirectoryPicker {
    entries: Vec<RankedDirectory>,
    matches: Vec<usize>,
    pub selected: usize,
    pub preview: Vec<PreviewEntry>,
    pub preview_offset: usize,
    pub preview_truncated: bool,
    pub preview_error: Option<String>,
    pub source_error: Option<String>,
    preview_path: Option<PathBuf>,
}

impl DirectoryPicker {
    /// One subprocess per opening. Filtering and moving the selection never
    /// invoke zoxide again or change the current working directory.
    pub fn load() -> Self {
        let output = std::process::Command::new("zoxide")
            .args(["query", "--list", "--score"])
            .output();
        match output {
            Ok(output) if output.status.success() => {
                Self::from_output(&String::from_utf8_lossy(&output.stdout))
            }
            Ok(output) => Self {
                source_error: Some(format!(
                    "zoxide query failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                ..Self::default()
            },
            Err(err) => Self {
                source_error: Some(format!("Cannot run zoxide: {err}")),
                ..Self::default()
            },
        }
    }
    pub(crate) fn from_output(output: &str) -> Self {
        let mut entries = Vec::new();
        for line in output.lines() {
            let line = line.trim_start();
            let Some(split) = line.find(char::is_whitespace) else {
                continue;
            };
            let Ok(score) = line[..split].parse::<f64>() else {
                continue;
            };
            let path = PathBuf::from(line[split..].trim_start());
            if score.is_finite() && path.is_absolute() {
                entries.push(RankedDirectory { score, path });
            }
        }
        // Stable sort retains zoxide order among equal scores.
        entries.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut picker = Self {
            entries,
            ..Self::default()
        };
        picker.filter("");
        picker
    }
    pub fn len(&self) -> usize {
        self.matches.len()
    }
    pub fn total(&self) -> usize {
        self.entries.len()
    }
    pub fn get(&self, index: usize) -> Option<&RankedDirectory> {
        self.matches.get(index).and_then(|&i| self.entries.get(i))
    }
    pub fn selected_dir(&self) -> Option<&RankedDirectory> {
        self.get(self.selected)
    }
    pub fn filter(&mut self, query: &str) {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.matches.clear();
        let mut fuzzy = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let path = entry.path.to_string_lossy().to_lowercase();
            if terms.iter().all(|term| path.contains(term.as_str())) {
                self.matches.push(i);
            } else if terms.iter().all(|term| subsequence(term, &path)) {
                fuzzy.push(i);
            }
        }
        // Prefer substring hits, and preserve the original rank within each tier.
        self.matches.extend(fuzzy);
        self.selected = 0;
        self.refresh_preview();
    }
    pub fn move_by(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.matches.len() - 1);
        self.refresh_preview();
    }
    pub fn scroll_preview(&mut self, delta: isize) {
        self.preview_offset = self
            .preview_offset
            .saturating_add_signed(delta)
            .min(self.preview.len().saturating_sub(1));
    }
    pub fn destination(&self) -> Result<String, String> {
        let dir = self
            .selected_dir()
            .ok_or_else(|| "No matching directory selected".to_owned())?;
        directory_filter(&dir.path.to_string_lossy())
    }
    fn refresh_preview(&mut self) {
        let path = self.selected_dir().map(|d| d.path.clone());
        if path == self.preview_path {
            return;
        }
        self.preview_path = path;
        self.preview.clear();
        self.preview_offset = 0;
        self.preview_truncated = false;
        self.preview_error = None;
        let Some(path) = self.preview_path.as_ref() else {
            return;
        };
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(err) => {
                self.preview_error = Some(format!("Cannot preview directory: {err}"));
                return;
            }
        };
        // Limit directory I/O. Only the selected directory is read, with no
        // recursion, file contents, image decoding, or stats for regular files.
        const LIMIT: usize = 256;
        for (i, entry) in entries.enumerate() {
            if i >= LIMIT {
                self.preview_truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry
                .file_type()
                .map(|kind| kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()))
                .unwrap_or(false);
            self.preview.push(PreviewEntry { name, is_dir });
        }
        self.preview.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
}

fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let mut next = chars.next();
    for c in haystack.chars() {
        if next == Some(c) {
            next = chars.next();
        }
        if next.is_none() {
            return true;
        }
    }
    next.is_none()
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

/// Move one directory up, undoing the path-prefix change made by Right.
/// Keep relative paths relative; at the filesystem root there is no parent.
pub fn parent_filter(input: &str) -> String {
    let (directory, _) = dir_and_fragment(input);
    if directory.is_empty() || directory == "./" {
        return "../".into();
    }
    let path = Path::new(&directory);
    if path.components().next_back() == Some(std::path::Component::ParentDir) {
        return format!("{directory}../");
    }
    let parent = if directory == "~/" {
        expand_home("~").parent().map(Path::to_path_buf)
    } else {
        path.parent().map(Path::to_path_buf)
    };
    let Some(parent) = parent else {
        return directory;
    };
    if parent.as_os_str().is_empty() {
        return String::new();
    }
    let mut text = parent.to_string_lossy().into_owned();
    if !text.ends_with('/') {
        text.push('/');
    }
    text
}

/// Byte cursor, always at a UTF-8 boundary. None represents the end.
pub fn cursor_position(text: &str, cursor: Option<usize>) -> usize {
    let mut pos = cursor.unwrap_or(text.len()).min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[derive(PartialEq, Eq, Debug)]
pub enum FilterEdit {
    Unhandled,
    Moved,
    Changed,
}

/// Small readline-style editor shared by the file and zoxide pickers.
/// Bare arrows are left to directory navigation in the file picker.
pub fn edit_filter(
    text: &mut String,
    cursor: &mut Option<usize>,
    key: crossterm::event::KeyCode,
    mods: crossterm::event::KeyModifiers,
    directory_query: bool,
) -> FilterEdit {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use FilterEdit::*;
    let pos = cursor_position(text, *cursor);
    let prev = || text[..pos].char_indices().next_back().map_or(0, |(i, _)| i);
    let next = || {
        text[pos..]
            .chars()
            .next()
            .map_or(pos, |c| pos + c.len_utf8())
    };
    let word_left = |space_only: bool| {
        let word = |c: char| {
            if space_only {
                !c.is_whitespace()
            } else {
                c.is_alphanumeric() || c == '_'
            }
        };
        let mut start = pos;
        let mut in_word = false;
        for (i, c) in text[..pos].char_indices().rev() {
            if word(c) {
                in_word = true;
            } else if in_word {
                break;
            }
            start = i;
        }
        start
    };
    let word_right = || {
        let mut end = pos;
        let mut in_word = false;
        for (i, c) in text[pos..].char_indices() {
            if !c.is_whitespace() {
                in_word = true;
            } else if in_word {
                break;
            }
            end = pos + i + c.len_utf8();
        }
        end
    };
    let ctrl = mods.contains(M::CONTROL);
    let alt = mods.contains(M::ALT);
    let plain = !mods.intersects(M::CONTROL | M::ALT | M::SUPER);
    let cmd = match key {
        K::Char(c) => c.to_ascii_lowercase(),
        _ => '\0',
    };
    let mut target = pos;
    let mut remove = None;
    if ctrl && cmd == 'u' {
        remove = Some(0..text.len());
        target = 0;
    } else if ctrl && cmd == 'w' {
        target = word_left(true);
        remove = Some(target..pos);
    } else if ctrl && cmd == 'g' {
        target = word_left(false);
        remove = Some(target..pos);
    } else if ctrl && cmd == 'k' {
        remove = Some(pos..text.len());
    } else if alt && cmd == 'd' {
        remove = Some(pos..word_right());
    } else if (ctrl && cmd == 'a') || key == K::Home {
        target = 0;
    } else if (ctrl && cmd == 'e') || key == K::End {
        target = text.len();
    } else if (ctrl && key == K::Left) || (alt && cmd == 'b') {
        target = word_left(true);
    } else if (ctrl && key == K::Right) || (alt && cmd == 'f') {
        target = word_right();
    } else if (ctrl && cmd == 'b') || (directory_query && plain && key == K::Left) {
        target = prev();
    } else if (ctrl && cmd == 'f') || (directory_query && plain && key == K::Right) {
        target = next();
    } else if key == K::Backspace {
        target = prev();
        remove = Some(target..pos);
    } else if key == K::Delete || (ctrl && cmd == 'd') {
        remove = Some(pos..next());
    } else if let K::Char(c) = key {
        if !plain || c.is_control() || (!directory_query && c == ' ') {
            return Unhandled;
        }
        let typed = if c == '-' && mods.contains(M::SHIFT) {
            '_'
        } else {
            c
        };
        if text.len() + typed.len_utf8() > 4096 {
            return Moved;
        }
        text.insert(pos, typed);
        *cursor = Some(pos + typed.len_utf8());
        return Changed;
    } else {
        return Unhandled;
    }
    if let Some(range) = remove {
        let changed = !range.is_empty();
        text.replace_range(range, "");
        *cursor = Some(target);
        if changed {
            Changed
        } else {
            Moved
        }
    } else {
        *cursor = Some(target);
        Moved
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

/// Scan once via fd, then filter cached paths without touching the filesystem.
#[derive(Clone)]
pub struct RecursiveFiles {
    pub root: PathBuf,
    files: std::rc::Rc<Vec<PathMatch>>,
}
impl RecursiveFiles {
    pub fn load(root: PathBuf) -> Result<Self, String> {
        let root = fs::canonicalize(&root).map_err(|e| format!("Cannot open search root: {e}"))?;
        let run = |exe: &str| {
            std::process::Command::new(exe)
                .args([
                    "--type",
                    "f",
                    "--print0",
                    "--absolute-path",
                    r"(?i)\.nii(\.gz)?$",
                ])
                .arg(&root)
                .output()
        };
        let output = match run("fd") {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => run("fdfind"),
            other => other,
        }
        .map_err(|e| format!("Cannot run fd: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "fd: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let mut files: Vec<_> = output
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .filter_map(|p| std::str::from_utf8(p).ok())
            .map(|p| PathMatch {
                path: p.to_owned(),
                is_dir: false,
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self {
            root,
            files: std::rc::Rc::new(files),
        })
    }
    pub fn filter(&self, query: &str) -> PathMatches {
        let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut exact = Vec::new();
        let mut fuzzy = Vec::new();
        for item in self.files.iter() {
            let relative = Path::new(&item.path)
                .strip_prefix(&self.root)
                .unwrap_or(Path::new(&item.path));
            let text = relative.to_string_lossy().to_lowercase();
            if terms.iter().all(|t| text.contains(t.as_str())) {
                exact.push(item.clone());
            } else if terms.iter().all(|t| subsequence(t, &text)) {
                fuzzy.push(item.clone());
            }
        }
        exact.extend(fuzzy);
        PathMatches {
            values: exact,
            selected: None,
            directory: self.root.to_string_lossy().into_owned(),
        }
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
    #[test]
    fn fd_result_filter_prefers_exact_and_preserves_paths_with_spaces() {
        let files = RecursiveFiles {
            root: PathBuf::from("/data"),
            files: std::rc::Rc::new(vec![
                PathMatch {
                    path: "/data/b r a i n.nii.gz".into(),
                    is_dir: false,
                },
                PathMatch {
                    path: "/data/first scan/Brain.nii.gz".into(),
                    is_dir: false,
                },
                PathMatch {
                    path: "/data/other.nii".into(),
                    is_dir: false,
                },
            ]),
        };
        let m = files.filter("brain");
        assert_eq!(m.values.len(), 2);
        assert_eq!(m.values[0].path, "/data/first scan/Brain.nii.gz");
        assert_eq!(files.filter("first brain").values.len(), 1);
        assert!(files.filter("absent").values.is_empty());
    }
    #[test]
    fn parent_navigation_reverses_directory_prefix() {
        assert_eq!(parent_filter("study/sub/"), "study/");
        assert_eq!(parent_filter("study/sub/brain"), "study/");
        assert_eq!(parent_filter("study/"), "");
        assert_eq!(parent_filter(""), "../");
        assert_eq!(parent_filter("../"), "../../");
        assert_eq!(parent_filter("/tmp/"), "/");
        assert_eq!(parent_filter("/"), "/");
    }

    #[test]
    fn filter_editor_uses_readline_keys_and_utf8_boundaries() {
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let mut text = "brain_map test".to_owned();
        let mut cursor = None;
        assert_eq!(
            edit_filter(&mut text, &mut cursor, K::Char('w'), M::CONTROL, true),
            FilterEdit::Changed
        );
        assert_eq!(text, "brain_map ");
        edit_filter(&mut text, &mut cursor, K::Char('a'), M::CONTROL, true);
        edit_filter(&mut text, &mut cursor, K::Char('脑'), M::NONE, true);
        assert_eq!(text, "脑brain_map ");
        edit_filter(&mut text, &mut cursor, K::Backspace, M::NONE, true);
        assert_eq!(text, "brain_map ");
        edit_filter(&mut text, &mut cursor, K::Char('f'), M::CONTROL, true);
        edit_filter(&mut text, &mut cursor, K::Char('k'), M::CONTROL, true);
        assert_eq!(text, "b");
        edit_filter(&mut text, &mut cursor, K::Char('u'), M::CONTROL, true);
        assert_eq!(text, "");
        assert_eq!(
            edit_filter(&mut text, &mut cursor, K::Left, M::NONE, false),
            FilterEdit::Unhandled
        );
        assert_eq!(
            edit_filter(&mut text, &mut cursor, K::Right, M::NONE, false),
            FilterEdit::Unhandled
        );
        assert_eq!(
            edit_filter(&mut text, &mut cursor, K::Char(' '), M::NONE, false),
            FilterEdit::Unhandled
        );
        edit_filter(&mut text, &mut cursor, K::Char('-'), M::SHIFT, false);
        assert_eq!(text, "_");
    }

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
    fn directory_picker_ranks_filters_and_previews_without_zoxide() {
        let root = test_directory();
        let second = root.join("Research folder with spaces");
        fs::create_dir(&second).unwrap();
        fs::write(second.join("sample.nii.gz"), b"").unwrap();
        let mut picker = DirectoryPicker::from_output(&format!(
            "  4.5 {}\n  30.0 {}\ninvalid line\n",
            second.display(),
            root.display()
        ));
        assert_eq!(picker.total(), 2);
        assert_eq!(picker.selected_dir().unwrap().path, root);
        assert!(picker
            .preview
            .iter()
            .any(|e| e.name == "groupA" && e.is_dir));
        assert!(picker
            .preview
            .iter()
            .any(|e| e.name == "brain_anterior_01.nii.gz"));
        picker.move_by(1);
        assert_eq!(picker.selected_dir().unwrap().path, second);
        assert_eq!(picker.preview[0].name, "sample.nii.gz");
        assert!(picker
            .destination()
            .unwrap()
            .ends_with("Research folder with spaces/"));
        picker.filter("RESEARCH spaces");
        assert_eq!(picker.len(), 1);
        assert_eq!(picker.selected_dir().unwrap().path, second);
        picker.filter("rsrch");
        assert_eq!(picker.len(), 1);
        picker.filter("zzzz-no-match");
        assert_eq!(picker.len(), 0);
        assert!(picker.preview.is_empty());
        assert!(picker.destination().is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn directory_preview_is_bounded_and_scrollable() {
        let root = test_directory();
        for i in 0..300 {
            fs::write(root.join(format!("file-{i}")), b"").unwrap();
        }
        let mut picker = DirectoryPicker::from_output(&format!("10 {}", root.display()));
        assert!(picker.preview_truncated);
        assert!(picker.preview.len() <= 256);
        picker.scroll_preview(5);
        assert_eq!(picker.preview_offset, 5);
        picker.scroll_preview(-999);
        assert_eq!(picker.preview_offset, 0);
        fs::remove_dir_all(root).unwrap();
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
        assert!(DirectoryPicker::default().destination().is_err());
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
