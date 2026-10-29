//! Pure path normalization and subtree checks (design 04 §1–§3).
//!
//! Paths keep their original case; only comparison copies are normalized.
//! `normalize` is best-effort and clamps absolute `..` at the root;
//! `try_normalize` reports E1003 for paths that try to escape their root.

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::fs::CaseSense;

pub fn normalize(path: &str) -> String {
    try_normalize(path).unwrap_or_else(|_| path.replace('\\', "/"))
}

pub fn try_normalize(path: &str) -> Result<String, KernelError> {
    let stripped = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!("//{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    };
    let slash = stripped.replace('\\', "/");
    let (root, rest) = split_root(&slash);
    let mut components: Vec<&str> = Vec::new();
    let absolute = !root.is_empty();

    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if matches!(components.last(), Some(last) if *last != "..") {
                    components.pop();
                } else if absolute {
                    return Err(KernelError::new(
                        "E1003",
                        "path escapes its root",
                        Some(path.to_string()),
                        false,
                    ));
                } else {
                    components.push("..");
                }
            }
            value => components.push(value),
        }
    }

    let mut result = String::new();
    match root.as_str() {
        "/" => {
            result.push('/');
            result.push_str(&components.join("/"));
        }
        "" => {
            if components.is_empty() {
                result.push('.');
            } else {
                result.push_str(&components.join("/"));
            }
        }
        root if root.starts_with("//") => {
            result.push_str(root);
            if !components.is_empty() {
                result.push('/');
                result.push_str(&components.join("/"));
            }
        }
        drive => {
            result.push_str(drive);
            if !components.is_empty() {
                result.push('/');
                result.push_str(&components.join("/"));
            }
        }
    }
    Ok(result)
}

fn split_root(path: &str) -> (String, &str) {
    if let Some(rest) = path.strip_prefix("//") {
        let mut parts = rest.splitn(3, '/');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let root = if share.is_empty() {
            format!("//{server}")
        } else {
            format!("//{server}/{share}")
        };
        return (root, rest);
    }
    if path.len() >= 2 {
        let bytes = path.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let rest = path[2..].strip_prefix('/').unwrap_or(&path[2..]);
            return (path[..2].to_string(), rest);
        }
    }
    if let Some(rest) = path.strip_prefix('/') {
        return ("/".to_string(), rest);
    }
    (String::new(), path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedDir {
    pub literal: String,
    pub canonical: Option<String>,
}

impl ProtectedDir {
    pub fn new(literal: impl Into<String>) -> Self {
        Self {
            literal: literal.into(),
            canonical: None,
        }
    }

    pub fn with_canonical(literal: impl Into<String>, canonical: Option<String>) -> Self {
        Self {
            literal: literal.into(),
            canonical,
        }
    }
}

/// Returns the configured protected entry hit by a path. The path should be
/// normalized; the protected entries are normalized here. Both the literal and
/// canonical forms are checked (design 04 §3.1).
pub fn protected_by(
    path: &str,
    canonical_path: Option<&str>,
    protected: &[ProtectedDir],
    sense: CaseSense,
) -> Option<String> {
    for entry in protected {
        if let Ok(literal) = try_normalize(&entry.literal) {
            if is_under(path, &literal, sense) {
                return Some(entry.literal.clone());
            }
        }
        if let (Some(canonical_path), Some(canonical_dir)) =
            (canonical_path, entry.canonical.as_deref())
        {
            if let (Ok(canonical_path), Ok(canonical_dir)) =
                (try_normalize(canonical_path), try_normalize(canonical_dir))
            {
                if is_under(&canonical_path, &canonical_dir, sense) {
                    return Some(entry.literal.clone());
                }
            }
        }
    }
    None
}

pub fn is_under(child: &str, ancestor: &str, sense: CaseSense) -> bool {
    let child = normalize(child);
    let ancestor = normalize(ancestor);
    let (child_root, child_parts) = split_root(&child);
    let (ancestor_root, ancestor_parts) = split_root(&ancestor);
    if !same_text(&child_root, &ancestor_root, sense) {
        return false;
    }
    let child_parts = if child_parts.is_empty() {
        Vec::new()
    } else {
        child_parts.split('/').collect::<Vec<_>>()
    };
    let ancestor_parts = if ancestor_parts.is_empty() {
        Vec::new()
    } else {
        ancestor_parts.split('/').collect::<Vec<_>>()
    };
    if child_parts.len() < ancestor_parts.len() {
        return false;
    }
    child_parts
        .iter()
        .zip(ancestor_parts.iter())
        .all(|(left, right)| same_text(left, right, sense))
}

fn same_text(left: &str, right: &str, sense: CaseSense) -> bool {
    match sense {
        CaseSense::Sensitive => left == right,
        CaseSense::CaseInsensitive => left.to_lowercase() == right.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_dot_segments_and_trailing_slash() {
        assert_eq!(normalize(r"C:\foo\\bar\."), "C:/foo/bar");
        assert_eq!(normalize("C:/foo/bar/"), "C:/foo/bar");
        assert_eq!(normalize("/foo/./bar"), "/foo/bar");
        assert_eq!(normalize(r"\\?\C:\foo\bar"), "C:/foo/bar");
        assert_eq!(normalize(r"\\?\UNC\server\share\foo"), "//server/share/foo");
    }

    #[test]
    fn resolves_parent_segments_without_crossing_root() {
        assert_eq!(normalize("/foo/bar/.."), "/foo");
        assert_eq!(normalize("foo/../bar"), "bar");
        assert_eq!(normalize("foo/../../bar"), "../bar");
        assert!(try_normalize("/../bar").is_err());
    }

    #[test]
    fn subtree_check_respects_component_boundaries() {
        assert!(is_under("/foo/bar", "/foo", CaseSense::Sensitive));
        assert!(is_under("/foo", "/foo", CaseSense::Sensitive));
        assert!(!is_under("/foobar", "/foo", CaseSense::Sensitive));
        assert!(!is_under("/foo", "/foo/bar", CaseSense::Sensitive));
        assert!(is_under("/", "/", CaseSense::Sensitive));
    }

    #[test]
    fn subtree_check_can_fold_case_for_windows_and_macos() {
        assert!(is_under(
            r"C:\Foo\Bar",
            r"C:\foo",
            CaseSense::CaseInsensitive
        ));
        assert!(!is_under(r"C:\Foo\Bar", r"C:\foo", CaseSense::Sensitive));
        assert!(!is_under(
            r"C:\foobar",
            r"C:\foo",
            CaseSense::CaseInsensitive
        ));
    }

    #[test]
    fn protected_check_uses_literal_and_canonical_forms() {
        let protected = vec![ProtectedDir::with_canonical(
            "/protected",
            Some("/real/protected".to_string()),
        )];
        assert_eq!(
            protected_by("/protected/file", None, &protected, CaseSense::Sensitive),
            Some("/protected".to_string())
        );
        assert_eq!(
            protected_by(
                "/link/file",
                Some("/real/protected/file"),
                &protected,
                CaseSense::Sensitive
            ),
            Some("/protected".to_string())
        );
        assert_eq!(
            protected_by("/public/file", None, &protected, CaseSense::Sensitive),
            None
        );
    }

    #[test]
    fn subtree_check_handles_unc_and_drive_roots() {
        assert!(is_under(
            r"\\server\share\dir\file",
            r"\\server\share",
            CaseSense::CaseInsensitive
        ));
        assert!(!is_under(
            r"\\server\share2\file",
            r"\\server\share",
            CaseSense::CaseInsensitive
        ));
        assert!(is_under(r"C:\file", "C:", CaseSense::CaseInsensitive));
    }
}
