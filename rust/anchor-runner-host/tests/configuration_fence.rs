mod common;

use std::path::Path;

const VARIABLES: [&str; 4] = [
    "ANCHOR_BWRAP",
    "ANCHOR_RUNNER_LIBRARY_ROOT",
    "ANCHOR_RUNNER_CATALOG_ROOT",
    "ANCHOR_RUNNER_STATE_ROOT",
];

// Match literal arguments passed to functions/macros, including env_path or an
// aliased reader. Longer error messages and documentation are not arguments for
// these variable names. This is a lexical fence, without constant evaluation.
fn count(_: &str, tokens: &[&str]) -> [usize; 4] {
    std::array::from_fn(|i| {
        let literal = format!("\"{}\"", VARIABLES[i]);
        tokens.iter().filter(|token| **token == literal).count()
    })
}

#[test]
fn deployment_environment_is_owned_by_main() {
    let counts = common::production_literal_counts(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        count,
    );
    assert_eq!(counts.get("main.rs"), Some(&[1; 4]));
    for (path, actual) in counts {
        if path != "main.rs" {
            assert_eq!(actual, [0; 4], "pass typed configuration into {path}");
        }
    }
}
