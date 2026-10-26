// H5 audit: retain literals, normalize only migrated paths and formatting commas.
#[allow(dead_code)]
mod common {
    include!("../../../rust/anchor-runner-host/tests/common/mod.rs");
    pub fn all_tokens(source: &str) -> Vec<&str> {
        tokens(source, true)
    }
}
use std::{
    fs,
    io::{self, Write},
    path::Path,
};
fn walk(path: &Path, out: &mut dyn Write) {
    for entry in fs::read_dir(path).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let s = fs::read_to_string(&p).unwrap();
        let tokens = common::all_tokens(&s);
        for i in 0..tokens.len().saturating_sub(2) {
            if ![
                "assert",
                "assert_eq",
                "assert_ne",
                "debug_assert",
                "debug_assert_eq",
                "debug_assert_ne",
            ]
            .contains(&tokens[i])
                || tokens[i + 1] != "!"
            {
                continue;
            }
            let mut depth = 0;
            let mut end = i + 2;
            loop {
                match tokens[end] {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth -= 1,
                    _ => {}
                };
                end += 1;
                if depth == 0 {
                    break;
                }
            }
            let mut normalized = Vec::new();
            let mut j = i;
            while j < end {
                let rest = &tokens[j..end];
                let mut replacement = None;
                for (pattern, name) in [
                    (
                        &["crate", ":", ":", "tool_names", ":", ":", "IMAGE_TOOL"][..],
                        "tool_names::IMAGE_TOOL",
                    ),
                    (
                        &["crate", ":", ":", "tool_names", ":", ":", "SEND_TOOL"][..],
                        "tool_names::SEND_TOOL",
                    ),
                    (
                        &["crate", ":", ":", "tool_names", ":", ":", "EDIT_TOOL_NAME"][..],
                        "tool_names::EDIT_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "tool_names", ":", ":", "READ_TOOL_NAME"][..],
                        "tool_names::READ_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "tool_names", ":", ":", "RUN_TOOL_NAME"][..],
                        "tool_names::RUN_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "node_tools", ":", ":", "RUN_TOOL_NAME"][..],
                        "tool_names::RUN_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "node_tools", ":", ":", "READ_TOOL_NAME"][..],
                        "tool_names::READ_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "node_tools", ":", ":", "EDIT_TOOL_NAME"][..],
                        "tool_names::EDIT_TOOL_NAME",
                    ),
                    (
                        &["crate", ":", ":", "channel_tools", ":", ":", "SEND_TOOL"][..],
                        "tool_names::SEND_TOOL",
                    ),
                    (
                        &["crate", ":", ":", "channel_tools", ":", ":", "IMAGE_TOOL"][..],
                        "tool_names::IMAGE_TOOL",
                    ),
                    (
                        &[
                            "crate",
                            ":",
                            ":",
                            "channel_replies",
                            ":",
                            ":",
                            "read_reply_images_at",
                        ][..],
                        "read_reply_images_at",
                    ),
                    (
                        &[
                            "crate",
                            ":",
                            ":",
                            "pilot_resources",
                            ":",
                            ":",
                            "release_development_scope",
                        ][..],
                        "release_development_scope",
                    ),
                    (
                        &[
                            "development_dependencies",
                            ":",
                            ":",
                            "release_development_scope",
                        ][..],
                        "release_development_scope",
                    ),
                    (
                        &[
                            "crate",
                            ":",
                            ":",
                            "pilot_tools",
                            ":",
                            ":",
                            "session_workspace",
                        ][..],
                        "session_workspace",
                    ),
                    (
                        &[
                            "crate",
                            ":",
                            ":",
                            "pilot_resources",
                            ":",
                            ":",
                            "session_workspace",
                        ][..],
                        "session_workspace",
                    ),
                ] {
                    if rest.starts_with(pattern) {
                        replacement = Some((pattern.len(), name));
                        break;
                    }
                }
                if let Some((len, name)) = replacement {
                    normalized.push(name);
                    j += len;
                    continue;
                }
                if tokens[j] != ","
                    || !tokens
                        .get(j + 1)
                        .is_some_and(|t| [")", "]", "}"].contains(t))
                {
                    normalized.push(tokens[j]);
                }
                j += 1;
            }
            out.write_all(normalized.join("\x1f").as_bytes()).unwrap();
            out.write_all(b"\x1e").unwrap();
        }
    }
}
fn main() {
    walk(
        Path::new(&std::env::args().nth(1).unwrap()),
        &mut io::stdout().lock(),
    );
}
