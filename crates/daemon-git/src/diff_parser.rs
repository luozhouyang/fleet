//! Unified diff parser.
//!
//! The parser understands the output of `git diff` with default settings,
//! including renames, copies, binary markers, quoted paths and missing trailing
//! newlines.

use fleet_core_protocol::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GitFileStatus};

use crate::error::{GitError, Result};

/// Maximum diff lines kept per file in the structured response.
pub(crate) const MAX_FILE_DIFF_LINES: usize = 5_000;

/// Maximum diff content bytes kept per file in the structured response.
pub(crate) const MAX_FILE_DIFF_CHARS: usize = 512 * 1024;

/// Parse a unified diff for a single file.
///
/// `path` is used when the diff carries no file header. When the input
/// contains several file sections, the section matching `path` is returned.
pub fn parse_unified_diff(path: &str, bytes: &[u8]) -> Result<FileDiff> {
    let files = parse_diff_files(bytes)?;
    if files.is_empty() {
        return Ok(FileDiff {
            path: path.to_string(),
            old_path: None,
            status: GitFileStatus::Modified,
            additions: 0,
            deletions: 0,
            is_binary: false,
            truncated: false,
            hunks: Vec::new(),
        });
    }

    let mut file = if files.len() == 1 {
        files.into_iter().next()
    } else {
        let path = path.to_string();
        files
            .into_iter()
            .find(|file| file.path == path || file.old_path.as_deref() == Some(path.as_str()))
    }
    .ok_or_else(|| GitError::InvalidOutput(format!("no diff section matched path `{path}`")))?;

    if file.path.is_empty() {
        file.path = path.to_string();
    }
    Ok(file)
}

/// Parse every file section in a unified diff.
pub(crate) fn parse_diff_files(bytes: &[u8]) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    let mut section: Option<Section> = None;

    for line in bytes.split(|byte| *byte == b'\n') {
        if line.starts_with(b"diff --git ") {
            if let Some(finished) = section.take() {
                files.push(finished.finish()?);
            }
            section = Some(Section::new(&line[b"diff --git ".len()..]));
            continue;
        }

        let Some(current) = section.as_mut() else {
            // Ignore preamble before the first file section.
            continue;
        };

        if line.starts_with(b"@@ ") {
            current.start_hunk(line)?;
            continue;
        }

        if current.hunk.is_some() {
            match line.first() {
                Some(b' ') => {
                    current.push_context(&line[1..]);
                    continue;
                }
                Some(b'+') => {
                    current.push_added(&line[1..]);
                    continue;
                }
                Some(b'-') => {
                    current.push_deleted(&line[1..]);
                    continue;
                }
                Some(b'\\') => {
                    current.push_no_newline(line);
                    continue;
                }
                _ => {
                    // The hunk ended; fall through to section headers.
                    current.finish_hunk();
                }
            }
        }

        current.handle_header(line)?;
    }

    if let Some(finished) = section {
        files.push(finished.finish()?);
    }
    Ok(files)
}

/// Builder for one `diff --git` section.
#[derive(Debug, Default)]
struct Section {
    path: Option<String>,
    old_path: Option<String>,
    status: Option<GitFileStatus>,
    is_binary: bool,
    hunks: Vec<DiffHunk>,
    additions: u32,
    deletions: u32,
    old_marker: Option<String>,
    new_marker: Option<String>,
    saw_old_marker: bool,
    saw_new_marker: bool,
    hunk: Option<HunkState>,
    /// Lines and bytes retained for the structured response.
    kept_lines: usize,
    kept_bytes: usize,
    /// Whether the per-file output cap was reached.
    truncated: bool,
}

/// A hunk currently being filled.
#[derive(Debug)]
struct HunkState {
    hunk: DiffHunk,
    old_line: u64,
    new_line: u64,
}

impl Section {
    fn new(rest: &[u8]) -> Self {
        let mut section = Self::default();
        if let Some((old, new)) = parse_git_header(&String::from_utf8_lossy(rest)) {
            if old != new {
                section.old_path = Some(old);
                section.status = Some(GitFileStatus::Renamed);
            }
            section.path = Some(new);
        }
        section
    }

    fn handle_header(&mut self, line: &[u8]) -> Result<()> {
        if line.starts_with(b"new file mode ") {
            self.status = Some(GitFileStatus::Added);
            return Ok(());
        }
        if line.starts_with(b"deleted file mode ") {
            self.status = Some(GitFileStatus::Deleted);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"rename from ") {
            self.old_path = Some(parse_header_value(&String::from_utf8_lossy(value)));
            self.status = Some(GitFileStatus::Renamed);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"rename to ") {
            self.path = Some(parse_header_value(&String::from_utf8_lossy(value)));
            self.status = Some(GitFileStatus::Renamed);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"copy from ") {
            self.old_path = Some(parse_header_value(&String::from_utf8_lossy(value)));
            self.status = Some(GitFileStatus::Copied);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"copy to ") {
            self.path = Some(parse_header_value(&String::from_utf8_lossy(value)));
            self.status = Some(GitFileStatus::Copied);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"--- ") {
            self.saw_old_marker = true;
            self.old_marker = parse_marker_path(&String::from_utf8_lossy(value), "a/");
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"+++ ") {
            self.saw_new_marker = true;
            self.new_marker = parse_marker_path(&String::from_utf8_lossy(value), "b/");
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"Binary files ") {
            self.is_binary = true;
            self.handle_binary_line(&String::from_utf8_lossy(value));
            return Ok(());
        }
        if line.starts_with(b"GIT binary patch") {
            self.is_binary = true;
            return Ok(());
        }
        // `index`, mode, similarity and similarity-related metadata lines do
        // not change the file model.
        Ok(())
    }

    fn handle_binary_line(&mut self, value: &str) {
        let value = value.strip_suffix(" differ").unwrap_or(value);
        let Some((old, new)) = value.split_once(" and ") else {
            return;
        };
        let old = parse_marker_path(old, "a/");
        let new = parse_marker_path(new, "b/");
        match (old, new) {
            (None, Some(new)) => {
                self.status = Some(GitFileStatus::Added);
                self.path = Some(new);
                self.old_path = None;
            }
            (Some(old), None) => {
                self.status = Some(GitFileStatus::Deleted);
                self.path = Some(old);
                self.old_path = None;
            }
            (Some(old), Some(new)) => {
                if self.path.is_none() {
                    self.path = Some(new);
                }
                if self.old_path.is_none() && old != self.path.clone().unwrap_or_default() {
                    self.old_path = Some(old);
                }
            }
            (None, None) => {}
        }
    }

    fn start_hunk(&mut self, line: &[u8]) -> Result<()> {
        self.finish_hunk();
        let hunk = parse_hunk_header(line)?;
        let old_line = hunk.old_start;
        let new_line = hunk.new_start;
        self.hunk = Some(HunkState {
            hunk,
            old_line,
            new_line,
        });
        Ok(())
    }

    fn finish_hunk(&mut self) {
        if let Some(state) = self.hunk.take() {
            self.hunks.push(state.hunk);
        }
    }

    /// Retain one line unless this file already reached its output cap.
    ///
    /// The cap is enforced while parsing so a huge patch never materializes
    /// the lines that were going to be truncated anyway. Line numbers and
    /// addition/deletion counters are updated by the callers regardless.
    fn push_line(&mut self, content: &[u8], make: impl FnOnce() -> DiffLine) {
        if self.hunk.is_none() {
            return;
        }
        if self.truncated
            || self.kept_lines >= MAX_FILE_DIFF_LINES
            || self.kept_bytes.saturating_add(content.len()) > MAX_FILE_DIFF_CHARS
        {
            self.truncated = true;
            return;
        }
        self.kept_lines += 1;
        self.kept_bytes += content.len();
        if let Some(state) = self.hunk.as_mut() {
            state.hunk.lines.push(make());
        }
    }

    fn push_context(&mut self, content: &[u8]) {
        let Some((old_line, new_line)) = self.hunk.as_mut().map(|state| {
            let lines = (state.old_line, state.new_line);
            state.old_line = state.old_line.saturating_add(1);
            state.new_line = state.new_line.saturating_add(1);
            lines
        }) else {
            return;
        };
        self.push_line(content, || DiffLine {
            line_type: DiffLineType::Context,
            content: decode(content),
            old_line_no: Some(old_line),
            new_line_no: Some(new_line),
        });
    }

    fn push_added(&mut self, content: &[u8]) {
        let Some(new_line) = self.hunk.as_mut().map(|state| {
            let line = state.new_line;
            state.new_line = state.new_line.saturating_add(1);
            line
        }) else {
            return;
        };
        self.additions = self.additions.saturating_add(1);
        self.push_line(content, || DiffLine {
            line_type: DiffLineType::Added,
            content: decode(content),
            old_line_no: None,
            new_line_no: Some(new_line),
        });
    }

    fn push_deleted(&mut self, content: &[u8]) {
        let Some(old_line) = self.hunk.as_mut().map(|state| {
            let line = state.old_line;
            state.old_line = state.old_line.saturating_add(1);
            line
        }) else {
            return;
        };
        self.deletions = self.deletions.saturating_add(1);
        self.push_line(content, || DiffLine {
            line_type: DiffLineType::Deleted,
            content: decode(content),
            old_line_no: Some(old_line),
            new_line_no: None,
        });
    }

    fn push_no_newline(&mut self, line: &[u8]) {
        self.push_line(line, || DiffLine {
            line_type: DiffLineType::NoNewline,
            content: decode(line),
            old_line_no: None,
            new_line_no: None,
        });
    }

    fn finish(mut self) -> Result<FileDiff> {
        self.finish_hunk();

        let mut status = self.status.unwrap_or(GitFileStatus::Modified);
        if self.saw_old_marker && self.old_marker.is_none() {
            status = GitFileStatus::Added;
        } else if self.saw_new_marker && self.new_marker.is_none() {
            status = GitFileStatus::Deleted;
        }

        let mut path = self.path.clone();
        match status {
            GitFileStatus::Added => {
                path = path.or(self.new_marker.clone());
                self.old_path = None;
            }
            GitFileStatus::Deleted => {
                path = path.or(self.old_marker.clone());
                self.old_path = None;
            }
            GitFileStatus::Renamed | GitFileStatus::Copied => {
                path = path.or(self.new_marker.clone()).or(self.old_marker.clone());
            }
            _ => {
                path = path.or(self.new_marker.clone()).or(self.old_marker.clone());
                self.old_path = None;
            }
        }

        let path = path.ok_or_else(|| {
            GitError::InvalidOutput("diff section is missing a file path".to_string())
        })?;

        Ok(FileDiff {
            path,
            old_path: self.old_path,
            status,
            additions: self.additions,
            deletions: self.deletions,
            is_binary: self.is_binary,
            truncated: self.truncated,
            hunks: self.hunks,
        })
    }
}

/// Parse `@@ -a,b +c,d @@ header`.
fn parse_hunk_header(line: &[u8]) -> Result<DiffHunk> {
    let text = String::from_utf8_lossy(line);
    let rest = text
        .strip_prefix("@@ ")
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk header: {text}")))?;
    let end = rest
        .find(" @@")
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk header: {text}")))?;
    let ranges = &rest[..end];
    let header = rest[end + " @@".len()..].trim();
    let mut parts = ranges.split_whitespace();
    let old_range = parts
        .next()
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk header: {text}")))?;
    let new_range = parts
        .next()
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk header: {text}")))?;
    let (old_start, old_lines) = parse_range(old_range, '-', &text)?;
    let (new_start, new_lines) = parse_range(new_range, '+', &text)?;

    Ok(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: if header.is_empty() {
            None
        } else {
            Some(header.to_string())
        },
        lines: Vec::new(),
    })
}

/// Parse `-start,count` or `+start,count`.
fn parse_range(field: &str, prefix: char, header: &str) -> Result<(u64, u64)> {
    let field = field
        .strip_prefix(prefix)
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk range in `{header}`")))?;
    let mut parts = field.split(',');
    let start = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed hunk range in `{header}`")))?;
    let lines = match parts.next() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| GitError::InvalidOutput(format!("malformed hunk range in `{header}`")))?,
        None => 1,
    };
    Ok((start, lines))
}

/// Parse the `a/path b/path` suffix of a `diff --git` header.
fn parse_git_header(rest: &str) -> Option<(String, String)> {
    if rest.starts_with('"') {
        let (old, consumed) = parse_c_quoted(rest)?;
        let rest = &rest[consumed..];
        let rest = rest.trim_start();
        let (new, _) = parse_c_quoted(rest)?;
        return Some((strip_prefix(&old, "a/"), strip_prefix(&new, "b/")));
    }

    let tokens: Vec<&str> = rest.split_whitespace().collect();
    if tokens.len() == 2 {
        return Some((strip_prefix(tokens[0], "a/"), strip_prefix(tokens[1], "b/")));
    }

    // Unquoted paths may contain spaces; ` b/` separates the two sides. Prefer
    // a split where both sides name the same file (the common modified case).
    let mut first = None;
    for (position, _) in rest.match_indices(" b/") {
        let old = strip_prefix(&rest[..position], "a/");
        let new = strip_prefix(&rest[position + 1..], "b/");
        if old == new {
            return Some((old, new));
        }
        if first.is_none() {
            first = Some((old, new));
        }
    }
    first
}

/// Parse a `---`/`+++`/binary marker path. `None` represents `/dev/null`.
fn parse_marker_path(value: &str, prefix: &str) -> Option<String> {
    let value = value.split('\t').next().unwrap_or(value);
    let value = value.trim_end_matches('\r');
    if value == "/dev/null" {
        return None;
    }
    let value = if value.starts_with('"') {
        parse_c_quoted(value).map(|(value, _)| value)?
    } else {
        value.to_string()
    };
    Some(strip_prefix(&value, prefix))
}

/// Parse a rename/copy header value, unquoting it when necessary.
fn parse_header_value(value: &str) -> String {
    let value = value.trim_end_matches('\r');
    if value.starts_with('"') {
        parse_c_quoted(value)
            .map(|(value, _)| value)
            .unwrap_or_else(|| value.to_string())
    } else {
        value.to_string()
    }
}

/// Strip a diff path prefix such as `a/` or `b/`.
fn strip_prefix(value: &str, prefix: &str) -> String {
    value.strip_prefix(prefix).unwrap_or(value).to_string()
}

/// Parse one C-style quoted path, returning the decoded value and bytes used.
fn parse_c_quoted(value: &str) -> Option<(String, usize)> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }

    let mut output = Vec::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                return Some((String::from_utf8_lossy(&output).into_owned(), index + 1));
            }
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'a' => output.push(0x07),
                    b'b' => output.push(0x08),
                    b't' => output.push(b'\t'),
                    b'n' => output.push(b'\n'),
                    b'v' => output.push(0x0b),
                    b'f' => output.push(0x0c),
                    b'r' => output.push(b'\r'),
                    b'"' => output.push(b'"'),
                    b'\\' => output.push(b'\\'),
                    b'0'..=b'7' => {
                        let mut octal = (escaped - b'0') as u16;
                        let mut digits = 1;
                        while digits < 3 {
                            match bytes.get(index + 1) {
                                Some(digit @ b'0'..=b'7') => {
                                    octal = octal * 8 + (digit - b'0') as u16;
                                    index += 1;
                                    digits += 1;
                                }
                                _ => break,
                            }
                        }
                        output.push(octal as u8);
                    }
                    other => output.push(other),
                }
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    None
}

/// Decode line content lossily.
fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = concat!(
        "diff --git a/src/lib.rs b/src/lib.rs\n",
        "index 1111111..2222222 100644\n",
        "--- a/src/lib.rs\n",
        "+++ b/src/lib.rs\n",
        "@@ -1,4 +1,5 @@ fn main\n",
        " line one\n",
        "-old line\n",
        "+new line\n",
        "+extra line\n",
        " line four\n",
    );

    #[test]
    fn parses_modified_file() {
        let file = parse_unified_diff("src/lib.rs", MODIFIED.as_bytes()).expect("parse");
        assert_eq!(file.path, "src/lib.rs");
        assert_eq!(file.old_path, None);
        assert_eq!(file.status, GitFileStatus::Modified);
        assert_eq!(file.additions, 2);
        assert_eq!(file.deletions, 1);
        assert!(!file.is_binary);
        assert_eq!(file.hunks.len(), 1);

        let hunk = &file.hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_lines, 4);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_lines, 5);
        assert_eq!(hunk.header.as_deref(), Some("fn main"));
        assert_eq!(hunk.lines.len(), 5);
        assert_eq!(hunk.lines[0].line_type, DiffLineType::Context);
        assert_eq!(hunk.lines[1].line_type, DiffLineType::Deleted);
        assert_eq!(hunk.lines[1].old_line_no, Some(2));
        assert_eq!(hunk.lines[1].new_line_no, None);
        assert_eq!(hunk.lines[2].line_type, DiffLineType::Added);
        assert_eq!(hunk.lines[2].new_line_no, Some(2));
    }

    #[test]
    fn parses_quoted_path() {
        let diff = b"diff --git \"a/\xC3\xA9.txt\" \"b/\xC3\xA9.txt\"\n\
new file mode 100644\n\
index 0000000..1234567\n\
--- /dev/null\n\
+++ \"b/\xC3\xA9.txt\"\n\
@@ -0,0 +1 @@\n\
+hello\n";
        let file = parse_unified_diff("\u{e9}.txt", diff).expect("parse");
        assert_eq!(file.path, "\u{e9}.txt");
        assert_eq!(file.status, GitFileStatus::Added);
        assert_eq!(file.additions, 1);
        assert_eq!(file.deletions, 0);
        assert_eq!(file.hunks[0].old_start, 0);
        assert_eq!(file.hunks[0].new_start, 1);
    }

    #[test]
    fn parses_rename_without_hunks() {
        let diff = b"diff --git a/old name.txt b/new name.txt\n\
similarity index 100%\n\
rename from old name.txt\n\
rename to new name.txt\n";
        let file = parse_unified_diff("new name.txt", diff).expect("parse");
        assert_eq!(file.path, "new name.txt");
        assert_eq!(file.old_path.as_deref(), Some("old name.txt"));
        assert_eq!(file.status, GitFileStatus::Renamed);
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn parses_deleted_file() {
        let diff = b"diff --git a/gone.txt b/gone.txt\n\
deleted file mode 100644\n\
index 1234567..0000000\n\
--- a/gone.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-bye\n";
        let file = parse_unified_diff("gone.txt", diff).expect("parse");
        assert_eq!(file.path, "gone.txt");
        assert_eq!(file.status, GitFileStatus::Deleted);
        assert_eq!(file.deletions, 1);
    }

    #[test]
    fn parses_binary_marker() {
        let diff = b"diff --git a/logo.png b/logo.png\n\
new file mode 100644\n\
index 0000000..1234567\n\
Binary files /dev/null and b/logo.png differ\n";
        let file = parse_unified_diff("logo.png", diff).expect("parse");
        assert!(file.is_binary);
        assert_eq!(file.status, GitFileStatus::Added);
        assert_eq!(file.path, "logo.png");
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn parses_no_newline_marker() {
        let diff = b"diff --git a/a.txt b/a.txt\n\
--- a/a.txt\n\
+++ b/a.txt\n\
@@ -1 +1 @@\n\
-old\n\
+new\n\
\\ No newline at end of file\n";
        let file = parse_unified_diff("a.txt", diff).expect("parse");
        let hunk = &file.hunks[0];
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(hunk.lines[2].line_type, DiffLineType::NoNewline);
    }

    #[test]
    fn parses_multiple_hunks_and_context_counts() {
        let diff = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1,2 +1,2 @@\n",
            " a\n",
            "-b\n",
            "+B\n",
            "@@ -10,2 +10,2 @@\n",
            " j\n",
            "-k\n",
            "+K\n",
        );
        let file = parse_unified_diff("a.txt", diff.as_bytes()).expect("parse");
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(file.hunks[1].old_start, 10);
        assert_eq!(file.hunks[1].lines[1].old_line_no, Some(11));
        assert_eq!(file.hunks[1].lines[2].new_line_no, Some(11));
        assert_eq!(file.additions, 2);
        assert_eq!(file.deletions, 2);
    }

    #[test]
    fn parses_mode_only_change_using_git_header() {
        let diff = b"diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        let file = parse_unified_diff("run.sh", diff).expect("parse");
        assert_eq!(file.path, "run.sh");
        assert_eq!(file.status, GitFileStatus::Modified);
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn malformed_hunk_header_is_an_error() {
        let diff = b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ nope\n";
        let error = parse_unified_diff("a.txt", diff).expect_err("must fail");
        assert!(matches!(error, GitError::InvalidOutput(_)));
    }

    #[test]
    fn truncates_lines_while_parsing() {
        let mut diff = String::from(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -0,0 +1,6000 @@\n",
        );
        for _ in 0..MAX_FILE_DIFF_LINES + 10 {
            diff.push_str("+x\n");
        }
        let file = parse_unified_diff("a.txt", diff.as_bytes()).expect("parse");
        assert!(file.truncated);
        assert_eq!(file.additions as usize, MAX_FILE_DIFF_LINES + 10);
        assert_eq!(file.hunks[0].lines.len(), MAX_FILE_DIFF_LINES);
    }

    #[test]
    fn multiple_sections_select_matching_path() {
        let diff = b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n\
diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-1\n+2\n";
        let file = parse_unified_diff("b.txt", diff).expect("parse");
        assert_eq!(file.path, "b.txt");
        assert_eq!(file.additions, 1);
    }
}
