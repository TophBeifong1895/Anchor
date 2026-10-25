//! Shared ASCII identifiers for Plugin references and filesystem-safe names.

/// A nonempty name using ASCII letters, digits, `_`, `.` and `-`, except `.`/`..`.
pub fn is_name(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

/// A Plugin id uses the name alphabet and starts with an ASCII letter or digit.
pub fn is_plugin_id(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && is_name(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_plugin_ids_have_distinct_prefix_contracts() {
        for value in ["a", "0", "A-b_c.d", "9-_."] {
            assert!(is_name(value), "{value:?}");
            assert!(is_plugin_id(value), "{value:?}");
        }
        for value in ["-x", "_x", ".x", "-", "_"] {
            assert!(is_name(value), "{value:?}");
            assert!(!is_plugin_id(value), "{value:?}");
        }
        for value in ["", ".", "..", "a/b", "a\\b", "a b", "a\n", "é", "中", "a\0"] {
            assert!(!is_name(value), "{value:?}");
            assert!(!is_plugin_id(value), "{value:?}");
        }
    }
}
