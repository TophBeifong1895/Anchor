//! Only this crate records grades: no other crate's source names the
//! grader principal, installs the grade validator or opens the grade root.
//! (The vocabulary itself lives in `anchor-facts`; tests may build fixtures.)

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_anchor_eval_records_grades() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut writers = Vec::new();
    for entry in std::fs::read_dir(workspace).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("anchor-") || name == "anchor-eval" || name == "anchor-facts" {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&entry.path().join("src"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            if [
                "anchor_facts::grader(",
                "install_grades(",
                "\"kernel-grades\"",
            ]
            .iter()
            .any(|needle| text.contains(needle))
            {
                writers.push(file.display().to_string());
            }
        }
    }
    assert!(writers.is_empty(), "{writers:?}");
}
