use std::{collections::BTreeMap, fs, path::Path};

const INTERNAL_TYPES: &[&str] = &[
    "RunStatus",
    "GraphRunRecord",
    "RunStore",
    "FileRunStore",
    "GraphRunner",
    "RunCursor",
];

// Lexical fence, not name resolution: count each exact identifier occurrence,
// including imports, qualified paths, alias declarations and macro token input.
// Alias uses and glob-imported names are not resolved; raw identifiers are counted.
// Comments (including nested block comments), strings (normal/raw/byte/C) and
// character literals are ignored. Lifetimes remain identifiers. Non-ASCII runs
// are conservatively treated as identifier continuations, including combining
// marks but not whitespace, so Unicode-affixed names are not ASCII false positives.
// Walk only this crate's src/**/*.rs, excluding tests/ and *tests.rs, plus external
// module files/subtrees declared under exact #[cfg(test)] (whitespace permitted).
// Skip only each guarded item/field, not the rest of its file, and resume scanning
// after its balanced delimiters. Other cfg expressions are deliberately counted;
// macros are not expanded. The runtime allowlist is exact files/prefixes below.
fn tokens(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let rest = &source[offset..];
        if rest.starts_with("//") {
            offset += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            offset += 2;
            let mut depth = 1;
            while offset < bytes.len() && depth > 0 {
                if bytes[offset..].starts_with(b"/*") {
                    depth += 1;
                    offset += 2;
                } else if bytes[offset..].starts_with(b"*/") {
                    depth -= 1;
                    offset += 2;
                } else {
                    offset += 1;
                }
            }
            assert_eq!(depth, 0, "unterminated block comment");
        } else if let Some(length) = literal_length(rest) {
            offset += length;
        } else {
            let character = rest.chars().next().unwrap();
            if character.is_whitespace() {
                offset += character.len_utf8();
            } else if identifier_part(character) {
                let start = offset;
                offset += rest
                    .char_indices()
                    .find(|(_, character)| !identifier_part(*character))
                    .map_or(rest.len(), |(index, _)| index);
                result.push(&source[start..offset]);
            } else {
                result.push(&source[offset..offset + character.len_utf8()]);
                offset += character.len_utf8();
            }
        }
    }
    result
}

fn identifier_part(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || character == '_'
        || (!character.is_ascii() && !character.is_whitespace())
}

fn literal_length(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let prefix = usize::from(matches!(bytes.first(), Some(b'b' | b'c')));
    if bytes.get(prefix) == Some(&b'r') {
        let mut quote = prefix + 1;
        while bytes.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if bytes.get(quote) == Some(&b'"') {
            let terminator = format!("\"{}", "#".repeat(quote - prefix - 1));
            let ending = source[quote + 1..]
                .find(&terminator)
                .expect("unterminated raw string");
            return Some(quote + 1 + ending + terminator.len());
        }
    }
    if bytes.get(prefix) == Some(&b'"') {
        let mut offset = prefix + 1;
        while offset < bytes.len() {
            match bytes[offset] {
                b'\\' => offset += 2,
                b'"' => return Some(offset + 1),
                _ => offset += 1,
            }
        }
        panic!("unterminated string");
    }
    if bytes.get(prefix) == Some(&b'\'') {
        let content = prefix + 1;
        let character = source[content..].chars().next()?;
        let end = if character == '\\' {
            let escape = *bytes.get(content + 1)?;
            match escape {
                b'x' => content + 4,
                b'u' => content + source[content..].find('}')? + 1,
                _ => content + 2,
            }
        } else {
            content + character.len_utf8()
        };
        if bytes.get(end) == Some(&b'\'') {
            return Some(end + 1);
        }
    }
    None
}

fn group_end(tokens: &[&str], start: usize) -> usize {
    let closing = match tokens[start] {
        "(" => ")",
        "[" => "]",
        "{" => "}",
        _ => panic!("expected opening delimiter"),
    };
    let mut offset = start + 1;
    while offset < tokens.len() {
        match tokens[offset] {
            token if token == closing => return offset + 1,
            "(" | "[" | "{" => offset = group_end(tokens, offset),
            _ => offset += 1,
        }
    }
    panic!("unbalanced delimiter");
}

fn production_tokens<'source>(
    source: &'source str,
    external_tests: &mut Vec<String>,
) -> Vec<&'source str> {
    let tokens = tokens(source);
    let mut production = Vec::new();
    let mut offset = 0;
    while offset < tokens.len() {
        if tokens[offset..].starts_with(&["#", "[", "cfg", "(", "test", ")", "]"]) {
            offset += 7;
            while tokens.get(offset..offset + 2) == Some(&["#", "["]) {
                offset = group_end(&tokens, offset + 1);
            }
            while offset < tokens.len() {
                match tokens[offset] {
                    "mod" if tokens.get(offset + 2) == Some(&";") => {
                        external_tests.push(tokens[offset + 1].to_owned());
                        offset += 3;
                        break;
                    }
                    ";" | "," => {
                        offset += 1;
                        break;
                    }
                    "{" => {
                        offset = group_end(&tokens, offset);
                        break;
                    }
                    "(" | "[" => offset = group_end(&tokens, offset),
                    _ => offset += 1,
                }
            }
        } else {
            production.push(tokens[offset]);
            offset += 1;
        }
    }
    production
}

fn count(source: &str, external_tests: &mut Vec<String>) -> usize {
    production_tokens(source, external_tests)
        .iter()
        .filter(|token| INTERNAL_TYPES.contains(token))
        .count()
}

fn runtime_file(path: &str) -> bool {
    matches!(
        path,
        "execution.rs" | "main.rs" | "node_host.rs" | "run_data.rs" | "application/run_access.rs"
    ) || path.starts_with("node_host/")
        || path.starts_with("application/run_access/")
}

fn source_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != "tests" {
                source_files(&path, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !entry.file_name().to_string_lossy().ends_with("tests.rs")
        {
            files.push(path);
        }
    }
}

fn check_count(
    path: &str,
    actual: usize,
    maximum: Option<usize>,
) -> Result<Option<String>, String> {
    let maximum = maximum.unwrap_or(0);
    if actual > maximum {
        Err(format!(
            "{path} {actual} (maximum {maximum}; extend the facade, not the baseline)"
        ))
    } else if actual < maximum {
        Ok(Some(format!(
            "run_access_fence: lower run-access-fence.txt: {path} {maximum} -> {actual}"
        )))
    } else {
        Ok(None)
    }
}

#[test]
fn run_access_fence() {
    assert_eq!(
        count(
            "RunStatus RunStatus::Running GraphRunRecord RunStore FileRunStore GraphRunner RunCursor",
            &mut Vec::new()
        ),
        7
    );
    assert_eq!(
        count(
            r####"// RunStatus
        /* RunStore /* GraphRunner */ RunCursor */
        "RunStatus \\" b"RunStore" c"GraphRunner" r###"RunCursor"### br#"FileRunStore"# cr"GraphRunRecord"
        '界' '\\' '\u{1234}' 'R' b'R'
        MyRunStatus RunStatusSuffix RunStatus界 界RunStatus RunStatuś
        use example::RunStatus as State; let _: State; r#RunCursor
    "####,
            &mut Vec::new()
        ),
        2
    );
    assert_eq!(
        count(
            r#"
        impl Host {
            fn before(_: RunStatus) {}
            #[cfg(test)] pub(crate) fn helper(_: RunStore) { let _ = GraphRunner; }
            fn after(_: GraphRunRecord) {}
        }
        #[cfg ( test )] mod first { use super::RunCursor; mod nested { type Alias = RunStore; } }
        fn between(_: FileRunStore) {}
        #[cfg(test)] mod second { let _ = "}"; }
        fn last(_: GraphRunner) {}
    "#,
            &mut Vec::new()
        ),
        4
    );
    let mut external_tests = Vec::new();
    assert_eq!(
        count(
            r#"
        #[cfg(test)] mod fixture;
        #[cfg(test)] use example::{RunStatus, RunStore};
        struct Host {
            #[cfg(test)] fixture: RunStore,
            production: GraphRunRecord,
        }
        #[cfg(test)] #[allow(dead_code)] fn helper() -> RunCursor { unreachable!() }
        type State = example::RunStatus;
        type Alias = State;
        check!(GraphRunner<RunCursor>);
    "#,
            &mut external_tests
        ),
        4
    );
    assert_eq!(external_tests, ["fixture"]);
    assert_eq!(
        count("RunStatus\u{3000}RunStore\u{a0}RunCursor", &mut Vec::new()),
        3
    );
    assert_eq!(
        check_count("known.rs", 2, Some(1)),
        Err("known.rs 2 (maximum 1; extend the facade, not the baseline)".to_owned())
    );
    assert_eq!(
        check_count("new.rs", 1, None),
        Err("new.rs 1 (maximum 0; extend the facade, not the baseline)".to_owned())
    );
    assert_eq!(
        check_count("known.rs", 1, Some(2)),
        Ok(Some(
            "run_access_fence: lower run-access-fence.txt: known.rs 2 -> 1".to_owned()
        ))
    );
    assert_eq!(
        check_count("deleted.rs", 0, Some(2)),
        Ok(Some(
            "run_access_fence: lower run-access-fence.txt: deleted.rs 2 -> 0".to_owned()
        ))
    );
    assert_eq!(check_count("known.rs", 1, Some(1)), Ok(None));
    assert_eq!(check_count("new.rs", 0, None), Ok(None));
    for allowed in [
        "execution.rs",
        "main.rs",
        "node_host.rs",
        "node_host/worker.rs",
        "run_data.rs",
        "application/run_access.rs",
        "application/run_access/views.rs",
    ] {
        assert!(runtime_file(allowed));
    }
    for business in [
        "execution/new.rs",
        "run_data/new.rs",
        "application/run_access_extra.rs",
        "node_host_extra.rs",
    ] {
        assert!(!runtime_file(business));
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = root.join("src");
    let mut files = Vec::new();
    source_files(&source_root, &mut files);
    files.sort();
    let mut counts = BTreeMap::new();
    let mut excluded_modules = Vec::new();
    for file in &files {
        let path = file
            .strip_prefix(&source_root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let mut external_tests = Vec::new();
        let occurrences = count(&fs::read_to_string(file).unwrap(), &mut external_tests);
        let module_directory = if file.file_name().unwrap() == "mod.rs" || path == "main.rs" {
            file.parent().unwrap().to_owned()
        } else {
            file.with_extension("")
        };
        for module in external_tests {
            excluded_modules.push(module_directory.join(module));
        }
        counts.insert(path, occurrences);
    }
    counts.retain(|path, _| {
        let file = source_root.join(path);
        !excluded_modules
            .iter()
            .any(|module| file == module.with_extension("rs") || file.starts_with(module))
    });

    let mut baseline = BTreeMap::new();
    for line in include_str!("../run-access-fence.txt").lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 2, "expected path count: {line}");
        let maximum: usize = fields[1].parse().expect("invalid baseline count");
        assert!(
            !runtime_file(fields[0]),
            "runtime file in business baseline: {line}"
        );
        assert!(
            baseline.insert(fields[0], maximum).is_none(),
            "duplicate baseline: {line}"
        );
    }
    let mut violations = Vec::new();
    let mut business_total = 0;
    let mut runtime_total = 0;
    for (path, actual) in &counts {
        if runtime_file(path) {
            runtime_total += actual;
            continue;
        }
        business_total += actual;
        if let Err(message) = check_count(path, *actual, baseline.get(path.as_str()).copied()) {
            violations.push(message);
        }
    }
    for (path, maximum) in baseline {
        let actual = counts.get(path).copied().unwrap_or(0);
        if let Ok(Some(message)) = check_count(path, actual, Some(maximum)) {
            eprintln!("{message}");
        }
    }
    eprintln!(
        "run_access_fence: {} production files; business {business_total}; runtime {runtime_total}",
        counts.len()
    );
    assert!(
        violations.is_empty(),
        "Run internals ratchet exceeded:\n{}",
        violations.join("\n")
    );
}
