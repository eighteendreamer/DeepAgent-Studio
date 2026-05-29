//! Secret scanning (开发计划.md Phase 9: Secret Scan).
//!
//! Detects likely hard-coded credentials in text/code before it is committed,
//! logged, or sent to a model. Detection is rule-based and dependency-free: each
//! [`SecretRule`] recognizes a well-known credential shape (AWS keys, private
//! key headers, bearer tokens, high-entropy assignments to secret-named vars).
//!
//! Findings deliberately **do not** echo the full secret value — only a masked
//! preview — so the scanner itself never leaks what it finds.

use serde::{Deserialize, Serialize};

/// Severity of a secret finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Possible secret; review recommended.
    Low,
    /// Likely secret.
    Medium,
    /// Almost certainly a live credential.
    High,
}

/// A detected potential secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretFinding {
    /// The rule that matched.
    pub rule: String,
    /// Severity.
    pub severity: Severity,
    /// 1-based line number.
    pub line: usize,
    /// A masked preview (never the full secret).
    pub masked: String,
}

/// A single detection rule.
struct SecretRule {
    name: &'static str,
    severity: Severity,
    /// Returns the matched secret substring if the line matches.
    matcher: fn(&str) -> Option<String>,
}

/// The secret scanner.
pub struct SecretScanner {
    rules: Vec<SecretRule>,
    /// Names that, when assigned a high-entropy string, are flagged.
    secret_var_hint: Vec<&'static str>,
}

impl Default for SecretScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretScanner {
    /// Build the scanner with the built-in rule set.
    pub fn new() -> Self {
        Self {
            rules: vec![
                SecretRule {
                    name: "aws_access_key_id",
                    severity: Severity::High,
                    matcher: match_aws_access_key,
                },
                SecretRule {
                    name: "private_key_block",
                    severity: Severity::High,
                    matcher: match_private_key,
                },
                SecretRule {
                    name: "bearer_token",
                    severity: Severity::Medium,
                    matcher: match_bearer,
                },
                SecretRule {
                    name: "slack_token",
                    severity: Severity::High,
                    matcher: match_slack,
                },
            ],
            secret_var_hint: vec![
                "password",
                "passwd",
                "secret",
                "api_key",
                "apikey",
                "token",
                "private_key",
                "access_key",
                "client_secret",
            ],
        }
    }

    /// Scan multi-line `content`, returning all findings.
    pub fn scan(&self, content: &str) -> Vec<SecretFinding> {
        let mut findings = Vec::new();
        for (i, line) in content.lines().enumerate() {
            let line_no = i + 1;
            for rule in &self.rules {
                if let Some(secret) = (rule.matcher)(line) {
                    findings.push(SecretFinding {
                        rule: rule.name.to_string(),
                        severity: rule.severity,
                        line: line_no,
                        masked: mask(&secret),
                    });
                }
            }
            if let Some(secret) = self.match_secret_assignment(line) {
                findings.push(SecretFinding {
                    rule: "high_entropy_secret_assignment".to_string(),
                    severity: Severity::Medium,
                    line: line_no,
                    masked: mask(&secret),
                });
            }
        }
        findings
    }

    /// Whether `content` contains any finding at or above `min`.
    pub fn has_secret(&self, content: &str, min: Severity) -> bool {
        self.scan(content).iter().any(|f| f.severity >= min)
    }

    /// Detect `SECRET_NAME = "high-entropy-value"` style assignments.
    fn match_secret_assignment(&self, line: &str) -> Option<String> {
        let lower = line.to_lowercase();
        // The variable name must hint at a secret.
        if !self.secret_var_hint.iter().any(|h| lower.contains(h)) {
            return None;
        }
        // Find a quoted value or a token after = / :.
        let value = extract_assigned_value(line)?;
        if value.len() >= 12 && shannon_entropy(&value) >= 3.0 {
            Some(value)
        } else {
            None
        }
    }
}

/// Mask a secret to a non-leaking preview: keep up to 3 leading chars.
fn mask(secret: &str) -> String {
    let visible: String = secret.chars().take(3).collect();
    format!("{visible}***({} chars)", secret.len())
}

fn extract_assigned_value(line: &str) -> Option<String> {
    // Prefer a quoted string.
    if let Some(start) = line.find(['"', '\'']) {
        let quote = line.as_bytes()[start] as char;
        if let Some(end_rel) = line[start + 1..].find(quote) {
            return Some(line[start + 1..start + 1 + end_rel].to_string());
        }
    }
    // Otherwise the token after = or :.
    let sep = line.find('=').or_else(|| line.find(':'))?;
    let rest = line[sep + 1..].trim();
    let token: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ';' && *c != ',')
        .collect();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn match_aws_access_key(line: &str) -> Option<String> {
    // AKIA + 16 uppercase alphanumerics.
    for token in line.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.len() == 20
            && (token.starts_with("AKIA") || token.starts_with("ASIA"))
            && token
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return Some(token.to_string());
        }
    }
    None
}

fn match_private_key(line: &str) -> Option<String> {
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY-----") {
        Some(line.trim().to_string())
    } else {
        None
    }
}

fn match_bearer(line: &str) -> Option<String> {
    let idx = line.find("Bearer ")?;
    let token: String = line[idx + 7..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    if token.len() >= 16 {
        Some(token)
    } else {
        None
    }
}

fn match_slack(line: &str) -> Option<String> {
    for token in line.split(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        if (token.starts_with("xoxb-") || token.starts_with("xoxp-")) && token.len() >= 16 {
            return Some(token.to_string());
        }
    }
    None
}

/// Shannon entropy (bits per char) — distinguishes random tokens from words.
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0u32) += 1;
    }
    let len = s.chars().count() as f64;
    counts
        .values()
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_aws_access_key() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan("aws_key = AKIAIOSFODNN7EXAMPLE");
        assert!(findings.iter().any(|f| f.rule == "aws_access_key_id"));
        assert!(findings.iter().any(|f| f.severity == Severity::High));
        // Value is masked.
        assert!(findings[0].masked.contains("***"));
        assert!(!findings[0].masked.contains("IOSFODNN"));
    }

    #[test]
    fn detects_private_key_header() {
        let scanner = SecretScanner::new();
        assert!(scanner.has_secret("-----BEGIN RSA PRIVATE KEY-----", Severity::High));
    }

    #[test]
    fn detects_bearer_token() {
        let scanner = SecretScanner::new();
        let f = scanner.scan("Authorization: Bearer abcdef0123456789ABCDEF");
        assert!(f.iter().any(|f| f.rule == "bearer_token"));
    }

    #[test]
    fn detects_slack_token() {
        let scanner = SecretScanner::new();
        assert!(scanner.has_secret("token = \"xoxb-123456789012-abcdef\"", Severity::High));
    }

    #[test]
    fn detects_high_entropy_secret_assignment() {
        let scanner = SecretScanner::new();
        let f = scanner.scan("API_KEY = \"a8Fk2Lm9Qw3Zx7Vb1Np5\"");
        assert!(f.iter().any(|f| f.rule == "high_entropy_secret_assignment"));
    }

    #[test]
    fn ignores_low_entropy_or_nonsecret_vars() {
        let scanner = SecretScanner::new();
        // Secret-named var but low-entropy/short value.
        assert!(scanner.scan("password = \"123\"").is_empty());
        // Non-secret var with a long value.
        assert!(scanner
            .scan("description = \"a long ordinary sentence here\"")
            .is_empty());
    }

    #[test]
    fn clean_code_has_no_findings() {
        let scanner = SecretScanner::new();
        let code = "fn main() {\n    println!(\"hello world\");\n}";
        assert!(scanner.scan(code).is_empty());
    }

    #[test]
    fn reports_correct_line_numbers() {
        let scanner = SecretScanner::new();
        let content = "line one\nline two\naws = AKIAIOSFODNN7EXAMPLE";
        let f = scanner.scan(content);
        assert_eq!(f[0].line, 3);
    }
}
