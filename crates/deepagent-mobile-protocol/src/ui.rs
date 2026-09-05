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
