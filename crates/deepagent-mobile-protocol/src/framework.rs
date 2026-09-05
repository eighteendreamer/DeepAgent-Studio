//! Multi-framework SDK protocol types.
//!
//! Defines the vocabulary for framework-specific debugging bridges:
//! uni-app/Vue, React Native, Compose, SwiftUI. Each framework exposes a
//! component tree, business events and network records through a uniform
//! protocol that the runtime can consume.

use serde::{Deserialize, Serialize};

/// Supported mobile UI frameworks.
///
/// Each variant represents a distinct runtime with its own component model,
/// debugging protocol and SDK distribution mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameworkKind {
    /// Native Android (View system).
    NativeAndroid,
    /// Native iOS (UIKit/AppKit).
    NativeIos,
    /// uni-app / Vue.js hybrid framework.
    UniApp,
    /// React Native JavaScript bridge.
    ReactNative,
    /// Jetpack Compose (Android declarative UI).
    Compose,
    /// SwiftUI (iOS declarative UI).
    SwiftUi,
}

impl FrameworkKind {
    /// Whether this framework runs on Android.
    pub fn is_android(&self) -> bool {
        matches!(
            self,
            Self::NativeAndroid | Self::UniApp | Self::ReactNative | Self::Compose
        )
    }

    /// Whether this framework runs on iOS.
    pub fn is_ios(&self) -> bool {
        matches!(
            self,
            Self::NativeIos | Self::UniApp | Self::ReactNative | Self::SwiftUi
        )
    }

    /// Whether this is a cross-platform hybrid framework.
    pub fn is_hybrid(&self) -> bool {
        matches!(self, Self::UniApp | Self::ReactNative)
    }

    /// Whether this is a native (non-hybrid) framework.
    pub fn is_native(&self) -> bool {
        !self.is_hybrid()
    }

    /// Display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::NativeAndroid => "Android Native",
            Self::NativeIos => "iOS Native",
            Self::UniApp => "uni-app / Vue",
            Self::ReactNative => "React Native",
            Self::Compose => "Jetpack Compose",
            Self::SwiftUi => "SwiftUI",
        }
    }
}

/// Debug profile configuration for an app.
///
/// SDK data is only active when the user explicitly enables a debug profile.
/// Release builds must default to `enabled: false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugProfile {
    /// Whether the debug profile is active.
    pub enabled: bool,
    /// Profile name (e.g., "dev", "staging").
    pub name: String,
    /// Framework this profile targets.
    pub framework: FrameworkKind,
    /// Package/bundle identifier.
    pub app_id: String,
    /// Optional SDK version constraint.
    pub sdk_version: Option<String>,
}

impl DebugProfile {
    /// Create a disabled profile (safe default for release builds).
    pub fn disabled(framework: FrameworkKind, app_id: &str) -> Self {
        Self {
            enabled: false,
            name: "release".into(),
            framework,
            app_id: app_id.into(),
            sdk_version: None,
        }
    }
}

/// A node in a framework-specific component tree.
///
/// Unlike the device-level `UiNode` (which comes from Accessibility/UI
/// Automator), component tree nodes come from the App SDK and expose
/// framework-specific details: Vue component names, React component names,
/// Compose composable names, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentNode {
    /// Unique ID within this component tree snapshot.
    pub node_id: String,
    /// Framework-specific type name (e.g., "View", "text", "ScrollView").
    pub component_type: String,
    /// Optional display label or accessibility text.
    pub label: Option<String>,
    /// Framework-specific props/attributes as JSON.
    pub props: Option<String>,
    /// Child node IDs (depth-first order).
    pub children: Vec<String>,
    /// Parent node ID (None for root).
    pub parent_id: Option<String>,
    /// Source location (file:line) if available.
    pub source_location: Option<String>,
}

/// A complete component tree snapshot from the App SDK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentTree {
    /// Snapshot ID (unique per capture).
    pub snapshot_id: String,
    /// Device ID this tree belongs to.
    pub device_id: String,
    /// Framework that produced this tree.
    pub framework: FrameworkKind,
    /// Root node ID.
    pub root_node_id: String,
    /// All nodes in the tree (keyed by node_id).
    pub nodes: Vec<ComponentNode>,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: u64,
}

impl ComponentTree {
    /// Find a node by ID.
    pub fn find_node(&self, node_id: &str) -> Option<&ComponentNode> {
        self.nodes.iter().find(|n| n.node_id == node_id)
    }

    /// Total node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Maximum depth from root.
    pub fn max_depth(&self) -> usize {
        fn depth(nodes: &[ComponentNode], node_id: &str) -> usize {
            nodes
                .iter()
                .find(|n| n.node_id == node_id)
                .map(|n| {
                    if n.children.is_empty() {
                        1
                    } else {
                        1 + n
                            .children
                            .iter()
                            .map(|c| depth(nodes, c))
                            .max()
                            .unwrap_or(0)
                    }
                })
                .unwrap_or(0)
        }
        depth(&self.nodes, &self.root_node_id)
    }

    /// Validate tree integrity: no dangling children, no duplicate IDs.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for node in &self.nodes {
            if !seen.insert(node.node_id.clone()) {
                return Err(format!("duplicate node_id: {}", node.node_id));
            }
        }
        for node in &self.nodes {
            for child_id in &node.children {
                if !seen.contains(child_id.as_str()) {
                    return Err(format!(
                        "dangling child {} in node {}",
                        child_id, node.node_id
                    ));
                }
            }
            if let Some(parent_id) = &node.parent_id {
                if !seen.contains(parent_id.as_str()) {
                    return Err(format!(
                        "dangling parent {} in node {}",
                        parent_id, node.node_id
                    ));
                }
            }
        }
        if !seen.contains(self.root_node_id.as_str()) {
            return Err(format!("root node {} not found", self.root_node_id));
        }
        Ok(())
    }
}

/// A business event from the App SDK.
///
/// Business events are app-level signals that don't fit the device-level
/// event model: navigation changes, state updates, custom analytics, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusinessEvent {
    /// Event ID (unique).
    pub event_id: String,
    /// Device ID.
    pub device_id: String,
    /// Framework that produced this event.
    pub framework: FrameworkKind,
    /// Event type (e.g., "navigation", "state_change", "custom").
    pub event_type: String,
    /// JSON payload.
    pub payload: String,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: u64,
}

/// SDK manifest for distribution and versioning.
///
/// Describes an SDK package that can be distributed to apps. Includes
/// version, supported frameworks, and compatibility constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkManifest {
    /// SDK identifier (e.g., "deepagent-mobile-sdk-android").
    pub sdk_id: String,
    /// Semantic version.
    pub version: String,
    /// Supported frameworks.
    pub frameworks: Vec<FrameworkKind>,
    /// Minimum app SDK version required.
    pub min_app_version: Option<String>,
    /// Distribution channel (e.g., "maven", "cocoapods", "npm").
    pub channel: String,
    /// Download URL or package reference.
    pub distribution_ref: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_kind_serde() {
        let kinds = [
            FrameworkKind::NativeAndroid,
            FrameworkKind::NativeIos,
            FrameworkKind::UniApp,
            FrameworkKind::ReactNative,
            FrameworkKind::Compose,
            FrameworkKind::SwiftUi,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let back: FrameworkKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn framework_kind_classification() {
        assert!(FrameworkKind::NativeAndroid.is_android());
        assert!(FrameworkKind::NativeAndroid.is_native());
        assert!(!FrameworkKind::NativeAndroid.is_hybrid());

        assert!(FrameworkKind::UniApp.is_hybrid());
        assert!(FrameworkKind::UniApp.is_android());
        assert!(FrameworkKind::UniApp.is_ios());

        assert!(FrameworkKind::ReactNative.is_hybrid());
        assert!(!FrameworkKind::ReactNative.is_native());

        assert!(FrameworkKind::Compose.is_android());
        assert!(!FrameworkKind::Compose.is_ios());

        assert!(FrameworkKind::SwiftUi.is_ios());
        assert!(!FrameworkKind::SwiftUi.is_android());
    }

    #[test]
    fn framework_display_names() {
        assert_eq!(FrameworkKind::UniApp.display_name(), "uni-app / Vue");
        assert_eq!(FrameworkKind::Compose.display_name(), "Jetpack Compose");
    }

    #[test]
    fn debug_profile_disabled_default() {
        let profile = DebugProfile::disabled(FrameworkKind::UniApp, "com.example.app");
        assert!(!profile.enabled);
        assert_eq!(profile.name, "release");
        assert_eq!(profile.framework, FrameworkKind::UniApp);
    }

    #[test]
    fn debug_profile_serde() {
        let profile = DebugProfile {
            enabled: true,
            name: "dev".into(),
            framework: FrameworkKind::ReactNative,
            app_id: "com.example.rn".into(),
            sdk_version: Some("1.0.0".into()),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let back: DebugProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn component_tree_find_and_count() {
        let tree = ComponentTree {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::UniApp,
            root_node_id: "root".into(),
            nodes: vec![
                ComponentNode {
                    node_id: "root".into(),
                    component_type: "page".into(),
                    label: None,
                    props: None,
                    children: vec!["child-1".into()],
                    parent_id: None,
                    source_location: None,
                },
                ComponentNode {
                    node_id: "child-1".into(),
                    component_type: "view".into(),
                    label: Some("Hello".into()),
                    props: Some(r#"{"class":"container"}"#.into()),
                    children: vec![],
                    parent_id: Some("root".into()),
                    source_location: Some("pages/index.vue:10".into()),
                },
            ],
            timestamp_ms: 1000,
        };

        assert_eq!(tree.node_count(), 2);
        assert!(tree.find_node("child-1").is_some());
        assert!(tree.find_node("nonexistent").is_none());
        assert_eq!(tree.max_depth(), 2);
    }

    #[test]
    fn component_tree_validate_ok() {
        let tree = ComponentTree {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::Compose,
            root_node_id: "root".into(),
            nodes: vec![
                ComponentNode {
                    node_id: "root".into(),
                    component_type: "Column".into(),
                    label: None,
                    props: None,
                    children: vec!["text-1".into()],
                    parent_id: None,
                    source_location: None,
                },
                ComponentNode {
                    node_id: "text-1".into(),
                    component_type: "Text".into(),
                    label: Some("Hello Compose".into()),
                    props: None,
                    children: vec![],
                    parent_id: Some("root".into()),
                    source_location: None,
                },
            ],
            timestamp_ms: 2000,
        };
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn component_tree_validate_duplicate_id() {
        let tree = ComponentTree {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::ReactNative,
            root_node_id: "root".into(),
            nodes: vec![
                ComponentNode {
                    node_id: "root".into(),
                    component_type: "View".into(),
                    label: None,
                    props: None,
                    children: vec![],
                    parent_id: None,
                    source_location: None,
                },
                ComponentNode {
                    node_id: "root".into(),
                    component_type: "Text".into(),
                    label: None,
                    props: None,
                    children: vec![],
                    parent_id: None,
                    source_location: None,
                },
            ],
            timestamp_ms: 3000,
        };
        assert!(tree.validate().is_err());
    }

    #[test]
    fn component_tree_validate_dangling_child() {
        let tree = ComponentTree {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::SwiftUi,
            root_node_id: "root".into(),
            nodes: vec![ComponentNode {
                node_id: "root".into(),
                component_type: "VStack".into(),
                label: None,
                props: None,
                children: vec!["ghost".into()],
                parent_id: None,
                source_location: None,
            }],
            timestamp_ms: 4000,
        };
        assert!(tree.validate().is_err());
    }

    #[test]
    fn component_tree_serde() {
        let tree = ComponentTree {
            snapshot_id: "snap-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::UniApp,
            root_node_id: "root".into(),
            nodes: vec![ComponentNode {
                node_id: "root".into(),
                component_type: "page".into(),
                label: None,
                props: None,
                children: vec![],
                parent_id: None,
                source_location: None,
            }],
            timestamp_ms: 5000,
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: ComponentTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    #[test]
    fn business_event_serde() {
        let event = BusinessEvent {
            event_id: "evt-1".into(),
            device_id: "dev-1".into(),
            framework: FrameworkKind::ReactNative,
            event_type: "navigation".into(),
            payload: r#"{"route":"/home"}"#.into(),
            timestamp_ms: 6000,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: BusinessEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn sdk_manifest_serde() {
        let manifest = SdkManifest {
            sdk_id: "deepagent-mobile-sdk-android".into(),
            version: "1.0.0".into(),
            frameworks: vec![FrameworkKind::NativeAndroid, FrameworkKind::Compose],
            min_app_version: Some("1.0.0".into()),
            channel: "maven".into(),
            distribution_ref: "com.deepagent:mobile-sdk:1.0.0".into(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let back: SdkManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, back);
    }
}
