mod common;

use std::path::Path;

const OWNED_TYPES: [&str; 2] = ["FilePluginCatalog", "Library"];

// Forbid the underlying types, including imports and alias declarations, so
// callers cannot bypass Host ownership with a renamed constructor. This is a
// lexical boundary, without name resolution or macro expansion.
fn count(_: &str, tokens: &[&str]) -> [usize; 2] {
    std::array::from_fn(|i| {
        tokens
            .iter()
            .filter(|token| **token == OWNED_TYPES[i])
            .count()
    })
}

#[test]
fn ownership_counting() {
    let source = r#"
        use anchor_graph_host::{FilePluginCatalog as OtherCatalog, PluginCatalog};
        use anchor_library::Library as OtherLibrary;
        anchor_graph_host::FilePluginCatalog::new(root);
        expose!(anchor_library::Library::new(root));
        // Library::new(root)
        let _ = "FilePluginCatalog::new(root)";
        #[cfg(test)] fn fixture() { Library::new(root); }
    "#;
    let tokens = common::production_tokens(source, &mut Vec::new());
    assert_eq!(count("service.rs", &tokens), [2, 2]);
}

#[test]
fn plugin_library_owns_catalog_and_library_construction() {
    let counts =
        common::production_counts(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), count);
    let mut violations = Vec::new();
    for (path, actual) in &counts {
        if path == "plugin_library.rs" || path.starts_with("plugin_library/") {
            continue;
        }
        for i in 0..OWNED_TYPES.len() {
            if actual[i] != 0 {
                violations.push(format!(
                    "{path}: {} {} (use plugin_library)",
                    OWNED_TYPES[i], actual[i]
                ));
            }
        }
    }
    eprintln!(
        "plugin_library_fence: {} production files; external owned types 0 / 0 required",
        counts.len()
    );
    assert!(
        violations.is_empty(),
        "Plugin ownership boundary exceeded:\n{}",
        violations.join("\n")
    );
}
