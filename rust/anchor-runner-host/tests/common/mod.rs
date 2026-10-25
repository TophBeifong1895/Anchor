use std::{collections::BTreeMap, fs, path::Path};

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
// macros are not expanded. Each fence defines its own exemptions and checks.
fn tokens(source: &str, retain_literals: bool) -> Vec<&str> {
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
            if retain_literals {
                result.push(&source[offset..offset + length]);
            }
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

pub fn production_tokens<'source>(
    source: &'source str,
    external_tests: &mut Vec<String>,
) -> Vec<&'source str> {
    production_tokens_inner(source, external_tests, false)
}

fn production_tokens_inner<'source>(
    source: &'source str,
    external_tests: &mut Vec<String>,
    retain_literals: bool,
) -> Vec<&'source str> {
    let tokens = tokens(source, retain_literals);
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

/// Apply a fence to the same production tokens and file exclusions.
#[allow(dead_code)]
pub fn production_counts<T>(
    source_root: &Path,
    count: impl Fn(&str, &[&str]) -> T,
) -> BTreeMap<String, T> {
    production_counts_inner(source_root, count, false)
}

/// Configuration fences also inspect literal arguments; ordinary fences ignore them.
#[allow(dead_code)]
pub fn production_literal_counts<T>(
    source_root: &Path,
    count: impl Fn(&str, &[&str]) -> T,
) -> BTreeMap<String, T> {
    production_counts_inner(source_root, count, true)
}

fn production_counts_inner<T>(
    source_root: &Path,
    count: impl Fn(&str, &[&str]) -> T,
    retain_literals: bool,
) -> BTreeMap<String, T> {
    let mut files = Vec::new();
    source_files(source_root, &mut files);
    files.sort();
    let mut counts = BTreeMap::new();
    let mut excluded_modules = Vec::new();
    for file in &files {
        let path = file
            .strip_prefix(source_root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let mut external_tests = Vec::new();
        let source = fs::read_to_string(file).unwrap();
        let tokens = if retain_literals {
            production_tokens_inner(&source, &mut external_tests, true)
        } else {
            production_tokens(&source, &mut external_tests)
        };
        let occurrences = count(&path, &tokens);
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

    counts
}
