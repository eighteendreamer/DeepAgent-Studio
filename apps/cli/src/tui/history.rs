use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Display columns a string occupies in the terminal, counting CJK as two.
pub fn display_width(text: &str) -> usize {
    text.chars().filter_map(UnicodeWidthChar::width).sum()
}

/// Cut a string to at most `width` display columns, marking the cut with one column.
pub fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut kept = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        kept.push(ch);
        used += ch_width;
    }
    kept.push('…');
    kept
}

/// A finished block of transcript.
///
/// Cells are immutable once created and are written into the terminal's own
/// scrollback, so the live viewport only ever paints the tail — the same split
/// codex-rs uses to keep streaming from flickering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryCell {
    UserPrompt { text: String },
}

impl HistoryCell {
    fn marker(&self) -> (&'static str, Color) {
        match self {
            Self::UserPrompt { .. } => ("> ", Color::Yellow),
        }
    }

    fn body(&self) -> &str {
        match self {
            Self::UserPrompt { text } => text,
        }
    }

    /// Renders the cell into at most `width` columns per line. The marker styles
    /// the first line only; wrapped lines are indented to stay aligned.
    pub fn to_lines(&self, width: u16) -> Vec<Line<'static>> {
        let (prefix_text, color) = self.marker();
        let style = Style::default().fg(color);
        let prefix_width = display_width(prefix_text);
        let indent = " ".repeat(prefix_width);
        let inner = (width as usize).saturating_sub(prefix_width).max(1);

        let mut lines: Vec<Line<'static>> = Vec::new();
        for paragraph in self.body().split('\n') {
            for wrapped in wrap(paragraph, inner) {
                let lead = if lines.is_empty() {
                    prefix_text
                } else {
                    &indent
                };
                lines.push(Line::from(vec![
                    Span::styled(lead.to_string(), style),
                    Span::styled(wrapped, style),
                ]));
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(prefix_text.to_string(), style)));
        }
        lines
    }
}

fn flush_line(lines: &mut Vec<String>, line: &mut String, line_width: &mut usize) {
    lines.push(std::mem::take(line));
    *line_width = 0;
}

/// Word wrap on terminal display columns.
///
/// Words wider than the line are hard-split per character, which is also the
/// correct path for CJK text that contains no spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let limit = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_width = 0usize;

    for paragraph in text.split('\n') {
        if paragraph.trim().is_empty() {
            if line_width > 0 {
                flush_line(&mut lines, &mut line, &mut line_width);
            }
            lines.push(String::new());
            continue;
        }
        for word in paragraph.split_whitespace() {
            let word_width = display_width(word);
            if word_width > limit {
                if line_width > 0 {
                    flush_line(&mut lines, &mut line, &mut line_width);
                }
                for ch in word.chars() {
                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if line_width + ch_width > limit {
                        flush_line(&mut lines, &mut line, &mut line_width);
                    }
                    line.push(ch);
                    line_width += ch_width;
                }
                continue;
            }
            let separator = usize::from(line_width > 0);
            if line_width + separator + word_width > limit {
                flush_line(&mut lines, &mut line, &mut line_width);
            }
            if line_width > 0 {
                line.push(' ');
                line_width += 1;
            }
            line.push_str(word);
            line_width += word_width;
        }
    }
    if line_width > 0 {
        flush_line(&mut lines, &mut line, &mut line_width);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{display_width, truncate, wrap, HistoryCell};

    fn rendered(line: &ratatui::text::Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn counts_cjk_as_two_columns() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("中文"), 4);
        assert_eq!(display_width("a中b"), 4);
    }

    #[test]
    fn wraps_on_word_boundaries() {
        let wrapped = wrap("hello brave world", 6);
        let wrapped: Vec<&str> = wrapped.iter().map(String::as_str).collect();
        assert_eq!(wrapped, vec!["hello", "brave", "world"]);
    }

    #[test]
    fn hard_splits_words_longer_than_the_line() {
        let wrapped = wrap("abcdefghij", 4);
        assert_eq!(wrapped, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wraps_cjk_without_spaces_by_display_columns() {
        let wrapped = wrap("终端代理界面", 4);
        assert_eq!(wrapped, vec!["终端", "代理", "界面"]);
        assert!(wrapped.iter().all(|line| display_width(line) <= 4));
    }

    #[test]
    fn keeps_blank_lines() {
        let wrapped = wrap("a\n\nb", 10);
        assert_eq!(wrapped, vec!["a", "", "b"]);
    }

    #[test]
    fn empty_body_still_occupies_one_line() {
        let lines = HistoryCell::UserPrompt {
            text: String::new(),
        }
        .to_lines(20);
        assert_eq!(lines.len(), 1);
        assert_eq!(rendered(&lines[0]), "> ");
    }

    #[test]
    fn marker_appears_once_and_lines_stay_inside_width() {
        let cell = HistoryCell::UserPrompt {
            text: "请解释一下这个渲染管线".to_string(),
        };
        let lines = cell.to_lines(8);
        assert_eq!(lines.len(), 4);
        assert_eq!(rendered(&lines[0]), "> 请解释");
        assert_eq!(rendered(&lines[1]), "  一下这");
        assert_eq!(rendered(&lines[2]), "  个渲染");
        assert_eq!(rendered(&lines[3]), "  管线");
        for line in &lines {
            assert!(
                display_width(&rendered(line)) <= 8,
                "line overflows the viewport: {}",
                rendered(line)
            );
        }
    }

    #[test]
    fn truncates_to_display_columns() {
        assert_eq!(truncate("workflow", 8), "workflow");
        assert_eq!(truncate("workflow", 6), "workf…");
        assert_eq!(display_width(&truncate("workflow", 6)), 6);
        assert_eq!(truncate("终端代理界面", 5), "终端…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(display_width(&truncate("终端代理界面", 5)), 5);
    }
}
