use serde::{Deserialize, Serialize};

/// A snapshot of the device UI hierarchy at a point in time.
///
/// `snapshot_id` is valid only until the next snapshot. All input operations
/// must carry the `snapshot_id` they were issued against; stale references
/// produce `StaleUiNode` errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub snapshot_id: String,
    pub device_id: String,
    pub root_node_id: String,
    pub nodes: Vec<UiNode>,
    pub captured_at_ms: u64,
}

impl UiSnapshot {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn max_depth(&self) -> u32 {
        if self.nodes.is_empty() {
            return 0;
        }
        let mut depth_map = std::collections::HashMap::new();
        depth_map.insert(self.root_node_id.clone(), 1u32);
        let mut max = 1u32;
        for node in &self.nodes {
            if let Some(&parent_depth) = depth_map.get(&node.node_id) {
                for child_id in &node.children {
                    depth_map.insert(child_id.clone(), parent_depth + 1);
                    max = max.max(parent_depth + 1);
                }
            }
        }
        max
    }

    pub fn find_node(&self, node_id: &str) -> Option<&UiNode> {
        self.nodes.iter().find(|n| n.node_id == node_id)
    }

    pub fn has_duplicate_ids(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.nodes.iter().any(|n| !seen.insert(&n.node_id))
    }

    pub fn has_dangling_children(&self) -> bool {
        let all_ids: std::collections::HashSet<&str> =
            self.nodes.iter().map(|n| n.node_id.as_str()).collect();
        self.nodes
            .iter()
            .flat_map(|n| &n.children)
            .any(|c| !all_ids.contains(c.as_str()))
    }

    /// Find all nodes matching the given filter criteria.
    ///
    /// All specified filter fields must match (AND logic). Unspecified fields
    /// are ignored. Text and content_desc matches are case-insensitive
    /// substring matches; resource_id is an exact match.
    pub fn find_by_filter(&self, filter: &super::NodeFilter) -> Vec<&UiNode> {
        self.nodes
            .iter()
            .filter(|node| {
                if let Some(ref text) = filter.text {
                    let node_text = node.text.as_deref().unwrap_or("");
                    let node_label = node.label.as_deref().unwrap_or("");
                    if !node_text.to_lowercase().contains(&text.to_lowercase())
                        && !node_label.to_lowercase().contains(&text.to_lowercase())
                    {
                        return false;
                    }
                }
                if let Some(ref rid) = filter.resource_id {
                    if node.accessibility_id.as_deref() != Some(rid.as_str()) {
                        return false;
                    }
                }
                if let Some(ref desc) = filter.content_desc {
                    let node_desc = node.accessibility_id.as_deref().unwrap_or("");
                    if !node_desc.to_lowercase().contains(&desc.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(ref role) = filter.role {
                    if node.role != *role {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    /// Redact sensitive fields in the snapshot (passwords, emails, phones).
    ///
    /// Returns a new snapshot with sensitive text replaced by placeholders.
    pub fn redact_sensitive(&self) -> UiSnapshot {
        let mut redacted = self.clone();
        for node in &mut redacted.nodes {
            if node.role == UiRole::Password {
                node.text = Some("[redacted:password]".into());
                node.label = None;
                node.accessibility_id = None;
                continue;
            }
            if let Some(ref text) = node.text {
                node.text = Some(redact_text(text));
            }
            if let Some(ref label) = node.label {
                node.label = Some(redact_text(label));
            }
            if let Some(ref desc) = node.accessibility_id {
                node.accessibility_id = Some(redact_text(desc));
            }
        }
        redacted
    }
}

/// Redact sensitive patterns in text.
///
/// Detects and replaces:
/// - Email addresses → `[redacted:email]`
/// - Phone numbers (10+ digits) → `[redacted:phone]`
/// - Password-like fields (node role is Password) → `[redacted:password]`
fn redact_text(text: &str) -> String {
    let result = redact_emails(text);
    redact_phones(&result)
}

/// Detect and redact email addresses in text (no regex dependency).
///
/// Finds substrings of the form `word@word.word` and replaces them.
fn redact_emails(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if let Some(at_pos) = find_email_start(&chars, i) {
            // Copy everything before the email
            let prefix: String = chars[i..at_pos].iter().collect();
            result.push_str(&prefix);

            // Find the end of the email
            if let Some(end) = find_email_end(&chars, at_pos) {
                result.push_str("[redacted:email]");
                i = end;
            } else {
                result.push(chars[at_pos]);
                i = at_pos + 1;
            }
        } else {
            let rest: String = chars[i..].iter().collect();
            result.push_str(&rest);
            break;
        }
    }
    result
}

fn find_email_start(chars: &[char], from: usize) -> Option<usize> {
    for i in from..chars.len() {
        if chars[i] == '@' && i > 0 && i < chars.len() - 1 {
            let local = &chars[..i];
            let has_local = local
                .iter()
                .rev()
                .take_while(|c| is_email_local(**c))
                .count()
                > 0;
            let domain = &chars[i + 1..];
            let has_domain = domain.iter().take_while(|c| is_email_domain(**c)).count() > 1;
            if has_local && has_domain {
                // Find start of local part
                let local_start = i - local
                    .iter()
                    .rev()
                    .take_while(|c| is_email_local(**c))
                    .count();
                return Some(local_start);
            }
        }
    }
    None
}

fn find_email_end(chars: &[char], at_pos: usize) -> Option<usize> {
    let domain_start = at_pos + 1;
    let domain_len = chars[domain_start..]
        .iter()
        .take_while(|c| is_email_domain(**c))
        .count();
    if domain_len > 1 {
        Some(domain_start + domain_len)
    } else {
        None
    }
}

fn is_email_local(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+'
}

fn is_email_domain(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '-'
}

/// Detect and redact phone numbers (10+ consecutive digits, ignoring
/// separators like spaces, dashes, parens).
fn redact_phones(text: &str) -> String {
    let digits: Vec<char> = text.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 10 {
        return text.to_string();
    }

    // Find contiguous runs of digit+separator characters
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut digit_count = 0;
            while i < len && (chars[i].is_ascii_digit() || is_phone_separator(chars[i])) {
                if chars[i].is_ascii_digit() {
                    digit_count += 1;
                }
                i += 1;
            }
            if digit_count >= 10 {
                result.push_str("[redacted:phone]");
            } else {
                let segment: String = chars[start..i].iter().collect();
                result.push_str(&segment);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn is_phone_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '(' | ')' | '.')
}

/// A single node in the unified UI tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub role: UiRole,
    pub text: Option<String>,
    pub label: Option<String>,
    pub accessibility_id: Option<String>,
    pub bounds: Bounds,
    pub visible: bool,
    pub enabled: bool,
    pub clickable: bool,
    pub editable: bool,
    pub children: Vec<String>,
    pub source: UiNodeSource,
}

/// Bounding rectangle in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Semantic role of a UI node, normalized across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRole {
    Page,
    Button,
    Text,
    TextBox,
    Password,
    Image,
    List,
    ListItem,
    Checkbox,
    Switch,
    Dialog,
    WebView,
    Unknown,
}

/// Origin of a UI node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiNodeSource {
    AndroidUiAutomator,
    IosXctest,
    AppSdk,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> UiSnapshot {
        UiSnapshot {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            root_node_id: "root".into(),
            captured_at_ms: 1000,
            nodes: vec![
                UiNode {
                    node_id: "root".into(),
                    parent_id: None,
                    role: UiRole::Page,
                    text: None,
                    label: None,
                    accessibility_id: None,
                    bounds: Bounds {
                        x: 0,
                        y: 0,
                        width: 1080,
                        height: 1920,
                    },
                    visible: true,
                    enabled: true,
                    clickable: false,
                    editable: false,
                    children: vec!["btn".into(), "txt".into()],
                    source: UiNodeSource::AndroidUiAutomator,
                },
                UiNode {
                    node_id: "btn".into(),
                    parent_id: Some("root".into()),
                    role: UiRole::Button,
                    text: Some("OK".into()),
                    label: None,
                    accessibility_id: None,
                    bounds: Bounds {
                        x: 100,
                        y: 200,
                        width: 200,
                        height: 60,
                    },
                    visible: true,
                    enabled: true,
                    clickable: true,
                    editable: false,
                    children: vec![],
                    source: UiNodeSource::AndroidUiAutomator,
                },
                UiNode {
                    node_id: "txt".into(),
                    parent_id: Some("root".into()),
                    role: UiRole::Text,
                    text: Some("Hello".into()),
                    label: None,
                    accessibility_id: None,
                    bounds: Bounds {
                        x: 100,
                        y: 300,
                        width: 300,
                        height: 40,
                    },
                    visible: true,
                    enabled: true,
                    clickable: false,
                    editable: false,
                    children: vec![],
                    source: UiNodeSource::AndroidUiAutomator,
                },
            ],
        }
    }

    #[test]
    fn snapshot_node_count_and_depth() {
        let snap = sample_snapshot();
        assert_eq!(snap.node_count(), 3);
        assert_eq!(snap.max_depth(), 2);
    }

    #[test]
    fn snapshot_no_duplicates_or_dangling() {
        let snap = sample_snapshot();
        assert!(!snap.has_duplicate_ids());
        assert!(!snap.has_dangling_children());
    }

    #[test]
    fn snapshot_find_node() {
        let snap = sample_snapshot();
        let node = snap.find_node("btn").unwrap();
        assert_eq!(node.role, UiRole::Button);
        assert!(snap.find_node("nonexistent").is_none());
    }

    #[test]
    fn ui_role_serde_stable() {
        let role = UiRole::WebView;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"web_view\"");
    }

    #[test]
    fn snapshot_serde_round_trip() {
        let snap = sample_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let back: UiSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }
}
