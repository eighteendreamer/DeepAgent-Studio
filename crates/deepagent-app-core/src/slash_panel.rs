//! Structured "panel" output for info-summary slash commands.
//!
//! Info-summary slash commands (`/mcp`, `/status`, `/hooks`, …) used to return a
//! flat text blob that the chat surface rendered as a plain message, mixing
//! servers with their tools, events with their rules, etc. To match Claude
//! Code's grouped/hierarchical presentation this module defines one generic
//! panel schema every such command builds, plus [`SlashPanel::to_fenced`] which
//! serializes it into a ` ```slash-panel ` fenced code block. The desktop
//! `MarkdownText` renderer recognizes that language (like `echarts`/`site-card`)
//! and draws a structured, status-badged, drill-down panel with a single
//! renderer — so every command gets consistent categorized display.

use serde::{Deserialize, Serialize};

/// A structured info panel: a title, optional subtitle, and one or more
/// sections of items. Serialized to JSON inside a `slash-panel` fenced block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashPanel {
    /// Panel heading (e.g. "MCP 服务器").
    pub title: String,
    /// Optional one-line summary under the title (e.g. "2/3 已启用").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Grouped sections. A single unnamed section is common for flat lists.
    pub sections: Vec<SlashSection>,
}

/// A group of items under an optional bold heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashSection {
    /// Optional bold group heading (e.g. an MCP scope, a hook event name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    /// Rows in this section.
    pub items: Vec<SlashPanelItem>,
}

/// One row in a panel section. `children` enables one level of drill-down
/// (server → tools, event → matchers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashPanelItem {
    /// Primary label (e.g. server / tool / setting name).
    pub label: String,
    /// Optional trailing value / description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Status accent: `"ok" | "warn" | "error" | "muted" | "info"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Small pill badges (e.g. "13 工具", "active").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<String>,
    /// Render the label in a monospace font (tool ids, paths, model ids).
    #[serde(default, skip_serializing_if = "is_false")]
    pub mono: bool,
    /// Nested rows (one level of hierarchy).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SlashPanelItem>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl SlashPanel {
    /// New panel with a title and no subtitle.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            sections: Vec::new(),
        }
    }

    /// Set the subtitle (builder style).
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Append a section (builder style).
    pub fn section(mut self, section: SlashSection) -> Self {
        self.sections.push(section);
        self
    }

    /// Append a single unnamed section holding `items` (builder style).
    pub fn items(mut self, items: Vec<SlashPanelItem>) -> Self {
        self.sections.push(SlashSection {
            heading: None,
            items,
        });
        self
    }

    /// Serialize into a ` ```slash-panel ` fenced code block for the chat
    /// surface. Falls back to the title text if serialization somehow fails.
    pub fn to_fenced(&self) -> String {
        match serde_json::to_string(self) {
            Ok(json) => format!("```slash-panel\n{json}\n```"),
            Err(_) => self.title.clone(),
        }
    }
}

impl SlashSection {
    /// A section with a heading.
    pub fn new(heading: impl Into<String>, items: Vec<SlashPanelItem>) -> Self {
        Self {
            heading: Some(heading.into()),
            items,
        }
    }
}

impl SlashPanelItem {
    /// A bare label-only row.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: None,
            status: None,
            badges: Vec::new(),
            mono: false,
            children: Vec::new(),
        }
    }

    /// Set the trailing value (builder style).
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Set the status accent (builder style).
    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Add a badge (builder style).
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badges.push(badge.into());
        self
    }

    /// Mark the label monospace (builder style).
    pub fn monospace(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Set nested children (builder style).
    pub fn children(mut self, children: Vec<SlashPanelItem>) -> Self {
        self.children = children;
        self
    }
}

/// Convenience: a label + value row (the common key-value shape).
pub fn kv(label: impl Into<String>, value: impl Into<String>) -> SlashPanelItem {
    SlashPanelItem::new(label).value(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_roundtrips_to_slash_panel_language() {
        let panel = SlashPanel::new("MCP 服务器")
            .subtitle("1/2 已启用")
            .items(vec![SlashPanelItem::new("everything")
                .status("ok")
                .value("stdio")
                .badge("13 工具")
                .children(vec![kv("echo", "Echoes back the input string")])]);
        let fenced = panel.to_fenced();
        assert!(fenced.starts_with("```slash-panel\n"));
        assert!(fenced.trim_end().ends_with("```"));
        // The inner JSON parses back to an equal panel.
        let inner = fenced
            .trim_start_matches("```slash-panel\n")
            .trim_end_matches("```")
            .trim();
        let parsed: SlashPanel = serde_json::from_str(inner).unwrap();
        assert_eq!(parsed, panel);
        assert_eq!(parsed.sections[0].items[0].children.len(), 1);
    }

    #[test]
    fn skips_empty_optional_fields_in_json() {
        let panel = SlashPanel::new("T").items(vec![SlashPanelItem::new("x")]);
        let json = serde_json::to_string(&panel).unwrap();
        // No subtitle, no value/status/badges/mono/children noise.
        assert!(!json.contains("subtitle"));
        assert!(!json.contains("badges"));
        assert!(!json.contains("children"));
        assert!(!json.contains("mono"));
    }
}
