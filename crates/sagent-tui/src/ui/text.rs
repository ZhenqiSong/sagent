//! 终端列宽与安全换行工具。
//!
//! Rust 字符串长度是 UTF-8 字节数，不能用于终端布局；这里统一按显示列宽截断，避免
//! 中文和 emoji 撑破 Ratatui 边框。编辑状态仍由 composer 的字符索引管理。

use unicode_width::UnicodeWidthChar;

/// 按终端显示列宽换行；长 token 没有空格时也会在字符边界强制折行。
pub fn wrap_display_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut width = 0;
    for character in text.chars() {
        if character == '\n' {
            lines.push(std::mem::take(&mut line));
            width = 0;
            continue;
        }
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width > 0 && width + character_width > max_width {
            lines.push(std::mem::take(&mut line));
            width = 0;
        }
        line.push(character);
        width += character_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::wrap_display_width;

    #[test]
    fn wraps_cjk_emoji_and_long_tokens_without_exceeding_display_width() {
        let lines = wrap_display_width("你好🙂abcdef", 4);

        assert!(
            lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 4)
        );
        assert_eq!(lines.concat(), "你好🙂abcdef");
    }

    #[test]
    fn preserves_explicit_newlines_and_handles_zero_width_safely() {
        assert_eq!(wrap_display_width("a\nb", 10), ["a", "b"]);
        assert!(wrap_display_width("内容", 0).is_empty());
    }
}
