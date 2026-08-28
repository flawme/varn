//! Ignore pattern matching for filesystem scanning.
//!
//! Varn supports `.varnignore` files with gitignore-style pattern matching.
//! This allows excluding paths like `target/`, `node_modules/`, or build
//! output from checkpoints.
//!
//! ## Pattern syntax
//!
//! Patterns follow a subset of gitignore semantics:
//!
//! - Blank lines and lines starting with `#` are ignored.
//! - A trailing `/` matches directories only.
//! - A leading `/` anchors the pattern to the root.
//! - `*` matches any sequence of characters except `/`.
//! - `**` matches any sequence of characters including `/`.
//! - `?` matches any single character except `/`.
//! - `[abc]` matches one character from the set.
//! - `[!abc]` matches one character NOT in the set.
//! - A leading `!` negates the pattern (re-includes a previously excluded path).
//!
//! Patterns are matched against paths relative to the scan root, using
//! forward slashes for cross-platform consistency.

use std::path::Path;

/// Result of checking a path against all ignore patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchResult {
    /// No pattern matched.
    NoMatch,
    /// A non-negated pattern matched (path should be ignored).
    Ignored,
    /// A negated pattern matched (path should be re-included).
    Negated,
}

/// A compiled ignore pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// The raw pattern string (without trailing newline).
    raw: String,
    /// Whether this pattern is negated (re-includes).
    negated: bool,
    /// Whether this pattern is directory-only (trailing `/`).
    dir_only: bool,
    /// Whether this pattern is anchored to the root (leading `/`).
    anchored: bool,
}

impl Pattern {
    /// Parse a single line into a compiled pattern.
    ///
    /// Returns `None` for blank lines and comments.
    pub fn parse(line: &str) -> Option<Self> {
        // Strip trailing whitespace (but not escaped whitespace).
        let line = line.trim_end_matches(['\r', '\n']);
        let line = line.trim_end();

        // Skip blank lines and comments.
        if line.is_empty() || line.starts_with('#') {
            return None;
        }

        let mut negated = false;
        let mut dir_only = false;
        let mut anchored = false;

        let mut pattern = line;

        // Check for negation.
        if let Some(rest) = pattern.strip_prefix('!') {
            negated = true;
            pattern = rest;
        }

        // Check for directory-only.
        if pattern.ends_with('/') {
            dir_only = true;
            pattern = &pattern[..pattern.len() - 1];
        }

        // Check for anchoring.
        if let Some(rest) = pattern.strip_prefix('/') {
            anchored = true;
            pattern = rest;
        }

        // If the pattern contains a `/` anywhere (not just leading/trailing),
        // it is implicitly anchored.
        if pattern.contains('/') {
            anchored = true;
        }

        Some(Self {
            raw: pattern.to_string(),
            negated,
            dir_only,
            anchored,
        })
    }

    /// Test whether this pattern matches the given relative path.
    ///
    /// `path` is relative to the scan root, using forward slashes.
    /// `is_dir` indicates whether the path is a directory.
    pub fn matches(&self, path: &str, is_dir: bool) -> bool {
        // Directory-only patterns don't match files.
        if self.dir_only && !is_dir {
            return false;
        }

        if self.anchored {
            // Anchored: match from the start of the path.
            glob_match(&self.raw, path)
        } else {
            // Unanchored: match against each path component suffix.
            // e.g. "*.log" matches "src/foo.log" because "foo.log" matches.
            let components: Vec<&str> = path.split('/').collect();
            for (i, _) in components.iter().enumerate() {
                let suffix = components[i..].join("/");
                if glob_match(&self.raw, &suffix) {
                    return true;
                }
            }
            false
        }
    }

    /// Whether this pattern re-includes (negates) a path.
    pub fn is_negated(&self) -> bool {
        self.negated
    }
}

/// A collection of ignore patterns loaded from one or more `.varnignore` files.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    patterns: Vec<Pattern>,
}

impl IgnoreRules {
    /// Create an empty set of rules.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load rules from a `.varnignore` file.
    ///
    /// If the file does not exist, returns an empty ruleset (no error).
    pub fn load_from_file(path: &Path) -> std::io::Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::new());
            }
            Err(e) => return Err(e),
        };
        Ok(Self::parse(&content))
    }

    /// Parse rules from a string.
    pub fn parse(content: &str) -> Self {
        let patterns = content.lines().filter_map(Pattern::parse).collect();
        Self { patterns }
    }

    /// Add a single pattern to this ruleset.
    pub fn add_pattern(&mut self, line: &str) {
        if let Some(p) = Pattern::parse(line) {
            self.patterns.push(p);
        }
    }

    /// Test whether the given path should be ignored.
    ///
    /// Patterns are evaluated in order. The last matching pattern wins:
    /// if a negated pattern matches after a non-negated one, the path is
    /// NOT ignored.
    ///
    /// If any parent directory of the path is ignored, the path itself is
    /// also ignored (gitignore semantics: ignoring a directory ignores its
    /// entire contents).
    pub fn is_ignored(&self, path: &str, is_dir: bool) -> bool {
        // First check if any parent directory is ignored.
        // If a parent is ignored, this path is ignored too (unless a
        // negation pattern explicitly re-includes it).
        let mut parent_ignored = false;
        let components: Vec<&str> = path.split('/').collect();
        for i in 1..components.len() {
            let parent = components[..i].join("/");
            if self.matches_patterns(&parent, true) == MatchResult::Ignored {
                parent_ignored = true;
            }
        }

        if parent_ignored {
            // Check if a negation re-includes this specific path.
            let self_match = self.matches_patterns(path, is_dir);
            // If the path itself is explicitly negated, it's not ignored.
            if self_match == MatchResult::Negated {
                return false;
            }
            return true;
        }

        self.matches_patterns(path, is_dir) == MatchResult::Ignored
    }

    /// Check all patterns against a path and return the match result.
    fn matches_patterns(&self, path: &str, is_dir: bool) -> MatchResult {
        let mut result = MatchResult::NoMatch;
        for pattern in &self.patterns {
            if pattern.matches(path, is_dir) {
                result = if pattern.is_negated() {
                    MatchResult::Negated
                } else {
                    MatchResult::Ignored
                };
            }
        }
        result
    }

    /// Whether there are no patterns loaded.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Number of patterns loaded.
    pub fn len(&self) -> usize {
        self.patterns.len()
    }
}

/// Glob-style pattern matching with `*`, `**`, `?`, and `[...]` support.
///
/// `*` matches any sequence of characters except `/`.
/// `**` matches any sequence of characters including `/`.
/// `?` matches any single character except `/`.
/// `[abc]` matches one character from the set.
/// `[!abc]` matches one character NOT in the set.
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes())
}

/// Inner recursive matcher operating on byte slices.
///
/// This uses recursion with backtracking to handle `*`, `**`, `?`, and
/// character classes. Recursion depth is bounded by the pattern length.
fn glob_match_inner(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;

    while ti < text.len() {
        if pi < pattern.len() {
            // Check for `**` (match across path separators).
            if pi + 1 < pattern.len() && pattern[pi] == b'*' && pattern[pi + 1] == b'*' {
                // Skip the `**` and any following `/`.
                let mut next_pi = pi + 2;
                if next_pi < pattern.len() && pattern[next_pi] == b'/' {
                    next_pi += 1;
                }
                // Try matching `**` against zero characters (skip it),
                // then against progressively more characters.
                // Zero: skip `**` and try the rest of the pattern.
                if glob_match_inner(&pattern[next_pi..], &text[ti..]) {
                    return true;
                }
                // One or more: consume one char and retry from `**` position
                // (which may consume more). But we advance `ti` and keep `pi`
                // at the `**` so it can match more.
                // Actually, we need to consume one char and try matching
                // the rest of the pattern after `**`.
                // Use recursion: `**` matches text[ti..] then rest of pattern.
                for skip in 1..=text.len() - ti {
                    if glob_match_inner(&pattern[next_pi..], &text[ti + skip..]) {
                        return true;
                    }
                }
                return false;
            }

            match pattern[pi] {
                b'*' => {
                    // `*` matches any sequence except `/`.
                    // Try matching zero chars first, then progressively more.
                    let next_pi = pi + 1;
                    // Zero chars.
                    if glob_match_inner(&pattern[next_pi..], &text[ti..]) {
                        return true;
                    }
                    // One or more chars (but not `/`).
                    for skip in 1..=text.len() - ti {
                        if text[ti + skip - 1] == b'/' {
                            break;
                        }
                        if glob_match_inner(&pattern[next_pi..], &text[ti + skip..]) {
                            return true;
                        }
                    }
                    return false;
                }
                b'?' => {
                    if text[ti] == b'/' {
                        return false;
                    }
                    pi += 1;
                    ti += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next_pi)) = match_char_class(pattern, pi, text[ti]) {
                        if matched {
                            pi = next_pi;
                            ti += 1;
                            continue;
                        }
                    }
                    return false;
                }
                c => {
                    if c == text[ti] {
                        pi += 1;
                        ti += 1;
                        continue;
                    }
                    return false;
                }
            }
        }

        // Pattern exhausted but text remains.
        return false;
    }

    // Text exhausted — check if remaining pattern is all `*`.
    while pi < pattern.len() {
        if pattern[pi] == b'*' {
            pi += 1;
        } else if pi + 1 < pattern.len() && pattern[pi] == b'*' && pattern[pi + 1] == b'*' {
            pi += 2;
        } else {
            break;
        }
    }

    pi == pattern.len()
}

/// Match a character class `[abc]` or `[!abc]` at `pattern[start]`.
///
/// Returns `Some((matched, next_index))` if the class was parsed successfully.
fn match_char_class(pattern: &[u8], start: usize, ch: u8) -> Option<(bool, usize)> {
    let mut i = start + 1; // Skip `[`.
    let mut negated = false;

    if i < pattern.len() && pattern[i] == b'!' {
        negated = true;
        i += 1;
    }

    let mut matched = false;
    let mut found_close = false;

    while i < pattern.len() {
        if pattern[i] == b']' {
            found_close = true;
            i += 1;
            break;
        }

        // Range: `a-z`.
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let lo = pattern[i];
            let hi = pattern[i + 2];
            if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if pattern[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }

    if !found_close {
        return None;
    }

    Some((matched ^ negated, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_blank_and_comment() {
        assert!(Pattern::parse("").is_none());
        assert!(Pattern::parse("   ").is_none());
        assert!(Pattern::parse("# comment").is_none());
    }

    #[test]
    fn parse_simple_pattern() {
        let p = Pattern::parse("*.log").unwrap();
        assert!(!p.negated);
        assert!(!p.dir_only);
        assert!(!p.anchored);
    }

    #[test]
    fn parse_negated_pattern() {
        let p = Pattern::parse("!important.log").unwrap();
        assert!(p.negated);
    }

    #[test]
    fn parse_directory_only() {
        let p = Pattern::parse("node_modules/").unwrap();
        assert!(p.dir_only);
        assert!(!p.anchored);
    }

    #[test]
    fn parse_anchored_pattern() {
        let p = Pattern::parse("/build").unwrap();
        assert!(p.anchored);
        assert!(!p.dir_only);
    }

    #[test]
    fn parse_anchored_by_slash_in_middle() {
        let p = Pattern::parse("src/build").unwrap();
        assert!(p.anchored);
    }

    #[test]
    fn glob_matches_simple() {
        assert!(glob_match("foo.txt", "foo.txt"));
        assert!(!glob_match("foo.txt", "bar.txt"));
    }

    #[test]
    fn glob_matches_star() {
        assert!(glob_match("*.log", "app.log"));
        assert!(!glob_match("*.log", "app.txt"));
        // `*` does not match `/`.
        assert!(!glob_match("*.log", "src/app.log"));
    }

    #[test]
    fn glob_matches_globstar() {
        assert!(glob_match("**/*.log", "app.log"));
        assert!(glob_match("**/*.log", "src/app.log"));
        assert!(glob_match("**/*.log", "src/nested/app.log"));
    }

    #[test]
    fn glob_matches_question_mark() {
        assert!(glob_match("file?.txt", "file1.txt"));
        assert!(!glob_match("file?.txt", "file12.txt"));
        assert!(!glob_match("file?.txt", "file.txt"));
    }

    #[test]
    fn glob_matches_char_class() {
        assert!(glob_match("file[abc].txt", "filea.txt"));
        assert!(glob_match("file[abc].txt", "fileb.txt"));
        assert!(!glob_match("file[abc].txt", "filed.txt"));
    }

    #[test]
    fn glob_matches_negated_char_class() {
        assert!(!glob_match("file[!abc].txt", "filea.txt"));
        assert!(glob_match("file[!abc].txt", "filed.txt"));
    }

    #[test]
    fn glob_matches_range() {
        assert!(glob_match("file[0-9].txt", "file5.txt"));
        assert!(!glob_match("file[0-9].txt", "filea.txt"));
    }

    #[test]
    fn ignore_rules_basic() {
        let rules = IgnoreRules::parse("*.log\ntarget/\n");
        assert!(rules.is_ignored("app.log", false));
        assert!(rules.is_ignored("target", true));
        assert!(!rules.is_ignored("src/main.rs", false));
    }

    #[test]
    fn ignore_rules_unanchored_matches_basename() {
        let rules = IgnoreRules::parse("*.log");
        assert!(rules.is_ignored("app.log", false));
        assert!(rules.is_ignored("src/app.log", false));
        assert!(rules.is_ignored("src/nested/app.log", false));
    }

    #[test]
    fn ignore_rules_anchored_only_matches_root() {
        let rules = IgnoreRules::parse("/build");
        assert!(rules.is_ignored("build", false));
        assert!(!rules.is_ignored("src/build", false));
    }

    #[test]
    fn ignore_rules_directory_only() {
        let rules = IgnoreRules::parse("build/");
        assert!(rules.is_ignored("build", true));
        assert!(!rules.is_ignored("build", false));
    }

    #[test]
    fn ignore_rules_negation() {
        let rules = IgnoreRules::parse("*.log\n!important.log");
        assert!(rules.is_ignored("app.log", false));
        assert!(!rules.is_ignored("important.log", false));
    }

    #[test]
    fn ignore_rules_negation_order_matters() {
        // If negation comes before the exclude, the exclude wins.
        let rules = IgnoreRules::parse("!important.log\n*.log");
        assert!(rules.is_ignored("important.log", false));
        assert!(rules.is_ignored("app.log", false));
    }

    #[test]
    fn ignore_rules_target_directory() {
        let rules = IgnoreRules::parse("target/");
        assert!(rules.is_ignored("target", true));
        assert!(rules.is_ignored("target/debug/varn", false));
    }

    #[test]
    fn ignore_rules_node_modules() {
        let rules = IgnoreRules::parse("node_modules/");
        assert!(rules.is_ignored("node_modules", true));
        assert!(rules.is_ignored("node_modules/express/index.js", false));
        assert!(!rules.is_ignored("src/main.rs", false));
    }

    #[test]
    fn ignore_rules_multiple_patterns() {
        let rules = IgnoreRules::parse(
            "# Build output\ntarget/\nnode_modules/\n*.log\n# Keep important\n!important.log\n",
        );
        assert!(rules.is_ignored("target", true));
        assert!(rules.is_ignored("node_modules", true));
        assert!(rules.is_ignored("debug.log", false));
        assert!(!rules.is_ignored("important.log", false));
        assert!(!rules.is_ignored("src/main.rs", false));
    }

    #[test]
    fn ignore_rules_empty() {
        let rules = IgnoreRules::new();
        assert!(rules.is_empty());
        assert!(!rules.is_ignored("anything", false));
    }

    #[test]
    fn ignore_rules_globstar_pattern() {
        let rules = IgnoreRules::parse("**/cache/");
        assert!(rules.is_ignored("cache", true));
        assert!(rules.is_ignored("src/cache", true));
        assert!(rules.is_ignored("src/nested/cache", true));
    }

    #[test]
    fn ignore_rules_load_from_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rules = IgnoreRules::load_from_file(&tmp.path().join(".varnignore")).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn ignore_rules_load_from_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".varnignore");
        std::fs::write(&path, "target/\n*.log\n").unwrap();
        let rules = IgnoreRules::load_from_file(&path).unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules.is_ignored("target", true));
        assert!(rules.is_ignored("app.log", false));
    }
}
