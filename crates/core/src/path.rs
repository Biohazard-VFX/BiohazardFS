//! Path normalization and namespace-name rules (FILESYSTEM_SEMANTICS.md).
//!
//! - Preserve filename case, but enforce case-insensitive sibling uniqueness
//!   by default. `case_insensitive_sibling_key` is the comparator for that.
//! - Relative workspace paths must reject absolute roots, `..` traversal,
//!   repeated separators, NUL/control characters, and path-shaped names.

use crate::error::CoreError;

/// Path separators rejected inside a single name segment.
const NAME_FORBIDDEN_CHARS: &[char] = &['/', '\\'];

/// Additional shell/filesystem metacharacters rejected in file names to keep
/// names portable across macOS/Windows/Linux (artist files move between them).
const NAME_FORBIDDEN_META_CHARS: &[char] = &[':', '*', '?', '"', '<', '>', '|'];

/// Maximum single-segment name length.
pub const MAX_NAME_LEN: usize = 255;

/// Validate a single namespace name segment (file or directory name).
pub fn validate_file_name(name: &str) -> Result<(), CoreError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(CoreError::new(
            "invalid_name",
            format!("name {name:?} is not allowed"),
        ));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(CoreError::new(
            "name_too_long",
            format!("name exceeds {MAX_NAME_LEN} characters"),
        ));
    }
    if let Some(ch) = name.chars().find(|ch| NAME_FORBIDDEN_CHARS.contains(ch)) {
        return Err(CoreError::new(
            "name_has_separator",
            format!("name {name:?} contains path separator {ch:?}"),
        ));
    }
    if let Some(ch) = name
        .chars()
        .find(|ch| NAME_FORBIDDEN_META_CHARS.contains(ch))
    {
        return Err(CoreError::new(
            "name_has_metachar",
            format!("name {name:?} contains forbidden character {ch:?}"),
        ));
    }
    if name.chars().any(|ch| ch.is_control()) {
        return Err(CoreError::new(
            "name_has_control_char",
            format!("name {name:?} contains a control character"),
        ));
    }
    if name != name.trim() {
        return Err(CoreError::new(
            "name_has_padding",
            format!("name {name:?} has leading or trailing whitespace"),
        ));
    }
    Ok(())
}

/// Normalize a relative workspace path: trim, reject absolute roots and `..`
/// traversal, collapse repeated separators, validate each segment, and return
/// the cleaned path (segments joined by `/`, no trailing slash unless empty).
pub fn normalize_relative_path(input: &str) -> Result<String, CoreError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(CoreError::new(
            "path_must_be_relative",
            "workspace paths must be relative, not absolute",
        ));
    }
    if trimmed.chars().any(|ch| ch == '\0') {
        return Err(CoreError::new(
            "path_contains_nul",
            "workspace path contains NUL",
        ));
    }
    let mut cleaned: Vec<&str> = Vec::new();
    for segment in trimmed.split(['/', '\\']) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(CoreError::new(
                "path_traversal_forbidden",
                "workspace paths must not contain parent traversal (..)",
            ));
        }
        validate_file_name(segment)?;
        cleaned.push(segment);
    }
    Ok(cleaned.join("/"))
}

/// Comparator key enforcing case-insensitive sibling uniqueness while
/// preserving the original name's case (FILESYSTEM_SEMANTICS.md). Also folds
/// common Unicode whitespace so visually identical names collide.
pub fn case_insensitive_sibling_key(name: &str) -> String {
    name.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_relative_paths() {
        assert_eq!(
            normalize_relative_path("Project/Shot010").unwrap(),
            "Project/Shot010"
        );
        assert_eq!(normalize_relative_path(" a / b ").unwrap(), "a/b");
        assert_eq!(normalize_relative_path("a//b").unwrap(), "a/b");
        assert_eq!(normalize_relative_path("./a/./b").unwrap(), "a/b");
        assert_eq!(normalize_relative_path("").unwrap(), "");
    }

    #[test]
    fn rejects_absolute_and_traversal_and_control() {
        assert_eq!(
            normalize_relative_path("/etc/passwd").unwrap_err().code,
            "path_must_be_relative"
        );
        assert_eq!(
            normalize_relative_path("../x").unwrap_err().code,
            "path_traversal_forbidden"
        );
        assert_eq!(
            normalize_relative_path("a/../b").unwrap_err().code,
            "path_traversal_forbidden"
        );
        assert_eq!(
            normalize_relative_path("a\u{0}b").unwrap_err().code,
            "path_contains_nul"
        );
    }

    #[test]
    fn rejects_bad_names() {
        assert_eq!(validate_file_name("").unwrap_err().code, "invalid_name");
        assert_eq!(validate_file_name("..").unwrap_err().code, "invalid_name");
        assert_eq!(
            validate_file_name("a/b").unwrap_err().code,
            "name_has_separator"
        );
        assert_eq!(
            validate_file_name("a*b").unwrap_err().code,
            "name_has_metachar"
        );
    }

    #[test]
    fn case_fold_key_collides_only_on_case() {
        assert_eq!(
            case_insensitive_sibling_key("Shot010"),
            case_insensitive_sibling_key("shot010")
        );
        assert_ne!(
            case_insensitive_sibling_key("Shot010"),
            case_insensitive_sibling_key("Shot011")
        );
    }
}
