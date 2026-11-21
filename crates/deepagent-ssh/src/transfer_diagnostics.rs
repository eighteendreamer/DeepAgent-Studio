//! Bound untrusted I/O descriptions before allocating application diagnostics.
//! Owned paths and mutation facts are recorded separately by their callers.
use std::fmt::{self, Write};

const DETAIL_BYTES: usize = 4096;
const TRUNCATED: &str = " [diagnostic truncated at 4096 UTF-8 bytes]";

struct Detail {
    text: String,
    truncated: bool,
}

impl Write for Detail {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.truncated {
            return Err(fmt::Error);
        }
        let remaining = DETAIL_BYTES - self.text.len();
        if value.len() <= remaining {
            self.text.push_str(value);
            return Ok(());
        }
        self.truncated = true;
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        Err(fmt::Error)
    }
}

pub(crate) fn error_detail(value: impl fmt::Display) -> String {
    let mut detail = Detail {
        text: String::with_capacity(DETAIL_BYTES),
        truncated: false,
    };
    let result = write!(&mut detail, "{value}");
    if detail.truncated {
        detail.text.push_str(TRUNCATED);
    } else if result.is_err() {
        detail.text.push_str(" [diagnostic formatting failed]");
    }
    detail.text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_and_exact_limit_diagnostics_remain_complete() {
        for text in [String::new(), "原始错误🙂".into(), "x".repeat(DETAIL_BYTES)] {
            assert_eq!(error_detail(&text), text);
        }
    }

    #[test]
    fn oversized_multibyte_diagnostics_keep_a_valid_prefix_and_explicit_marker() {
        let prefix = "x".repeat(DETAIL_BYTES - 1);
        let result = error_detail(format_args!("{}{}", prefix, "🙂".repeat(DETAIL_BYTES)));
        assert_eq!(result, format!("{prefix}{TRUNCATED}"));
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn formatter_failure_is_explicit_instead_of_panicking_or_claiming_completeness() {
        struct Broken;
        impl fmt::Display for Broken {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("partial")?;
                Err(fmt::Error)
            }
        }
        assert_eq!(
            error_detail(Broken),
            "partial [diagnostic formatting failed]"
        );
    }
}
