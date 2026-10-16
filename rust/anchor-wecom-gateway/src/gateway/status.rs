//! Presentation of Host progress steps inside one chat bubble.
//!
//! The Host reports a stable category token and, for tool steps, a step number.
//! The transport decides how that looks on this platform: a single line that
//! updates in place. The final answer, the superseded notice and the failure
//! notice never pass through here, so their wording stays exactly as written.

/// Category used when no Host category is known, e.g. the placeholder line.
pub(super) const DEFAULT: &str = "default";

/// Longest rendered status line. The prefix is never truncated.
const MAX_CHARS: usize = 200;

/// Render one status line, e.g. `第 2 步 · 🔍 正在查看文件`.
///
/// An absent or unknown category contributes no icon, and only tool steps carry
/// the step marker, so the leading "preparing" item stays `⏳ 正在准备`.
/// The client owns its loading indicator; no animated dots are added here.
pub(super) fn render(category: Option<&str>, step: Option<u32>, content: &str) -> String {
    let content = single_line(content);
    let content = content.trim_end_matches(|character: char| {
        character == '.' || character == '…' || character.is_whitespace()
    });
    let mut prefix = String::new();
    if let Some(step) = step {
        prefix.push_str(&format!("第 {step} 步 · "));
    }
    if let Some(icon) = category.and_then(icon) {
        prefix.push_str(icon);
        prefix.push(' ');
    }
    let budget = MAX_CHARS.saturating_sub(prefix.chars().count());
    let body: String = content.chars().take(budget).collect();
    format!("{prefix}{body}")
}

/// Icons are presentation only: an unknown or missing category stays bare.
fn icon(category: &str) -> Option<&'static str> {
    match category {
        "preparing" | "default" => Some("⏳"),
        "read_file" => Some("🔍"),
        "command" => Some("⚙️"),
        "history" => Some("🗂️"),
        "image" => Some("🖼️"),
        "plugin" => Some("🔎"),
        "final_result" => Some("✍️"),
        _ => None,
    }
}

/// Collapse anything a status line must not contain: the bubble is a single
/// line, so control characters become separators instead of line breaks.
fn single_line(content: &str) -> String {
    content
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_renders_its_icon_and_step_marker() {
        assert_eq!(
            render(Some("read_file"), Some(2), "正在查看文件"),
            "第 2 步 · 🔍 正在查看文件"
        );
        assert_eq!(
            render(Some("command"), Some(1), "正在工作区执行命令"),
            "第 1 步 · ⚙️ 正在工作区执行命令"
        );
        assert_eq!(
            render(Some("history"), Some(3), "正在查看历史记录"),
            "第 3 步 · 🗂️ 正在查看历史记录"
        );
        assert_eq!(
            render(Some("image"), Some(4), "正在准备图片"),
            "第 4 步 · 🖼️ 正在准备图片"
        );
        assert_eq!(
            render(Some("plugin"), Some(5), "正在查询业务系统"),
            "第 5 步 · 🔎 正在查询业务系统"
        );
        assert_eq!(
            render(Some("final_result"), Some(6), "正在整理回复"),
            "第 6 步 · ✍️ 正在整理回复"
        );
        assert_eq!(
            render(Some("default"), Some(7), "正在处理"),
            "第 7 步 · ⏳ 正在处理"
        );
    }

    #[test]
    fn guidance_and_legacy_lines_keep_their_shape() {
        // The preparing item has no step number.
        assert_eq!(render(Some("preparing"), None, "正在准备…"), "⏳ 正在准备");
        // A legacy Host sends no category: no icon, no marker, still one line.
        assert_eq!(render(None, None, "正在处理…"), "正在处理");
        // Normalise legacy trailing dots; loading is shown by the client.
        for content in ["正在处理", "正在处理…", "正在处理...", "正在处理. . . "]
        {
            assert_eq!(render(None, None, content), "正在处理");
        }
    }

    #[test]
    fn unknown_categories_and_control_characters_stay_harmless() {
        assert_eq!(
            render(Some("mystery"), Some(1), "正在处理"),
            "第 1 步 · 正在处理"
        );
        assert_eq!(
            render(Some("read_file"), Some(1), "正在查看\n文件\tnow"),
            "第 1 步 · 🔍 正在查看 文件 now"
        );
    }

    #[test]
    fn long_content_is_truncated_without_touching_the_prefix() {
        let long = "正".repeat(400);
        let rendered = render(Some("read_file"), Some(12), &long);
        assert_eq!(rendered.chars().count(), MAX_CHARS);
        assert!(rendered.starts_with("第 12 步 · 🔍 正"));
        // Exactly at the boundary nothing is lost.
        let exact = "正".repeat(MAX_CHARS - "第 1 步 · 🔍 ".chars().count());
        assert_eq!(
            render(Some("read_file"), Some(1), &exact).chars().count(),
            MAX_CHARS
        );
    }
}
