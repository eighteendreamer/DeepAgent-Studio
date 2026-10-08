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
    UserPrompt {
        text: String,
    },
    Assistant {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Tool {
        /// Correlation id, so the completion row can update the row it started.
        call_id: String,
        name: String,
        summary: String,
        outcome: String,
    },
    Notice {
        text: String,
    },
    Failure {
        text: String,
    },
}

impl HistoryCell {
    fn marker(&self) -> (&'static str, Color) {
        match self {
            Self::UserPrompt { .. } => ("> ", Color::Yellow),
            Self::Assistant { .. } => ("  ", Color::White),
            Self::Reasoning { .. } => ("  ", Color::DarkGray),
            Self::Tool { .. } => ("* ", Color::Cyan),
            Self::Notice { .. } => ("* ", Color::DarkGray),
            Self::Failure { .. } => ("! ", Color::Red),
        }
    }

    fn body(&self) -> String {
        match self {
            Self::UserPrompt { text }
            | Self::Assistant { text }
            | Self::Reasoning { text }
            | Self::Notice { text }
            | Self::Failure { text } => text.clone(),
            Self::Tool {
                name,
                summary,
                outcome,
                ..
            } => {
                let summary = if summary.is_empty() {
                    String::new()
                } else {
                    format!(": {summary}")
                };
                format!("{name}{summary} — {outcome}")
            }
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
    use ratatui::text::Line;

    fn rendered(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn lines_of(cell: &HistoryCell, width: u16) -> Vec<String> {
        cell.to_lines(width).iter().map(rendered).collect()
    }

    #[test]
    fn counts_cjk_as_two_columns() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("中文"), 4);
        assert_eq!(display_width("a中b"), 4);
    }

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(
            wrap("hello brave world", 6),
            vec!["hello", "brave", "world"]
        );
    }

    #[test]
    fn hard_splits_words_longer_than_the_line() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wraps_cjk_without_spaces_by_display_columns() {
        let wrapped = wrap("终端代理界面", 4);
        assert_eq!(wrapped, vec!["终端", "代理", "界面"]);
        assert!(wrapped.iter().all(|line| display_width(line) <= 4));
    }

    #[test]
    fn keeps_blank_lines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn truncates_to_display_columns() {
        assert_eq!(truncate("workflow", 8), "workflow");
        assert_eq!(truncate("workflow", 6), "workf…");
        assert_eq!(display_width(&truncate("workflow", 6)), 6);
        assert_eq!(truncate("终端代理界面", 5), "终端…");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn prompt_marker_appears_once_and_lines_stay_inside_width() {
        let lines = lines_of(
            &HistoryCell::UserPrompt {
                text: "请解释一下这个渲染管线".to_string(),
            },
            8,
        );
        assert_eq!(lines, vec!["> 请解释", "  一下这", "  个渲染", "  管线"]);
    }

    #[test]
    fn empty_body_still_occupies_one_line() {
        let lines = lines_of(
            &HistoryCell::Notice {
                text: String::new(),
            },
            20,
        );
        assert_eq!(lines, vec!["* "]);
    }

    #[test]
    fn assistant_and_reasoning_are_flat_text_without_a_box() {
        assert_eq!(
            lines_of(
                &HistoryCell::Assistant {
                    text: "先看内核再改".to_string()
                },
                20
            ),
            vec!["  先看内核再改"]
        );
        assert_eq!(
            lines_of(
                &HistoryCell::Reasoning {
                    text: "step by step".to_string()
                },
                20
            ),
            vec!["  step by step"]
        );
    }

    #[test]
    fn tool_cell_names_the_tool_with_summary_and_outcome() {
        let lines = lines_of(
            &HistoryCell::Tool {
                call_id: "c1".to_string(),
                name: "read_file".to_string(),
                summary: "src/main.rs".to_string(),
                outcome: "done in 12ms".to_string(),
            },
            40,
        );
        assert_eq!(lines, vec!["* read_file: src/main.rs — done in 12ms"]);
    }

    #[test]
    fn tool_cell_without_summary_still_reads_cleanly() {
        let lines = lines_of(
            &HistoryCell::Tool {
                call_id: "c2".to_string(),
                name: "bash".to_string(),
                summary: String::new(),
                outcome: "failed".to_string(),
            },
            40,
        );
        assert_eq!(lines, vec!["* bash — failed"]);
    }

    #[test]
    fn failure_uses_the_warning_marker() {
        assert_eq!(
            lines_of(
                &HistoryCell::Failure {
                    text: "run failed".to_string()
                },
                40
            ),
            vec!["! run failed"]
        );
    }
}
