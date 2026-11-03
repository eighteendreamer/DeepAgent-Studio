//! v1 branch catalog shared by graph admission and condition evaluation.
//! The desktop projects the same checked-in policy, without another run chain.
use std::collections::BTreeSet;
use std::sync::OnceLock;

use deepagent_core::error::{CoreError, Result};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Port {
    id: String,
    label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    single_case: Port,
    fallback: Port,
    reserved: Vec<String>,
}

fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let policy: Policy = serde_json::from_str(include_str!("condition-branches.v1.json"))
            .expect("checked-in condition branch policy");
        assert_eq!(policy.version, 1);
        assert!(!policy.single_case.label.is_empty() && !policy.fallback.label.is_empty());
        policy
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BranchContract {
    /// Ordered alongside condition groups; a singleton maps to the legacy true port.
    pub cases: Vec<String>,
    pub fallback: String,
}

impl BranchContract {
    pub(super) fn compile(config: &Map<String, Value>) -> Result<Self> {
        let invalid = |reason| CoreError::invalid(format!("ConditionConfigInvalid: {reason}"));
        let groups = config
            .get("conditions")
            .and_then(Value::as_array)
            .filter(|groups| !groups.is_empty())
            .ok_or_else(|| invalid("requires nonempty conditions"))?;
        let policy = policy();
        let mut ids = BTreeSet::new();
        let mut cases = Vec::with_capacity(groups.len());
        for group in groups {
            let id = group
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("condition group requires an ID"))?;
            super::graph::validate_id(id, "condition group ID")?;
            // `true` is an unambiguous legacy singleton ID, not a second case.
            if policy.reserved.iter().any(|reserved| reserved == id)
                && !(groups.len() == 1 && id == policy.single_case.id)
            {
                return Err(invalid(
                    "condition group ID conflicts with a reserved branch",
                ));
            }
            if !ids.insert(id) {
                return Err(invalid("duplicate condition group IDs"));
            }
            cases.push(if groups.len() == 1 {
                policy.single_case.id.clone()
            } else {
                id.to_owned()
            });
        }
        Ok(Self {
            cases,
            fallback: policy.fallback.id.clone(),
        })
    }

    pub(super) fn handles(&self) -> BTreeSet<String> {
        self.cases
            .iter()
            .chain(std::iter::once(&self.fallback))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h199_branch_catalog_is_nonempty_unambiguous_and_includes_its_fallback() {
        for (ids, expected) in [
            (vec!["if"], vec!["true", "false"]),
            (vec!["true"], vec!["true", "false"]),
            (vec!["a", "b", "c"], vec!["a", "b", "c", "false"]),
        ] {
            let config = serde_json::json!({"conditions":ids.iter().map(|id| serde_json::json!({"id":id})).collect::<Vec<_>>()});
            let contract = BranchContract::compile(config.as_object().unwrap()).unwrap();
            assert_eq!(
                contract.handles(),
                expected.iter().map(|s| s.to_string()).collect()
            );
            assert!(contract
                .cases
                .iter()
                .all(|id| contract.handles().contains(id)));
            assert!(contract.handles().contains(&contract.fallback));
        }
        for ids in [
            vec![],
            vec!["false"],
            vec!["true", "b"],
            vec!["a", "true"],
            vec!["a", "false"],
            vec!["a", "a"],
            vec!["bad/id"],
        ] {
            let config = serde_json::json!({"conditions":ids.iter().map(|id| serde_json::json!({"id":id})).collect::<Vec<_>>()});
            assert!(
                BranchContract::compile(config.as_object().unwrap()).is_err(),
                "{ids:?}"
            );
        }
    }
}
