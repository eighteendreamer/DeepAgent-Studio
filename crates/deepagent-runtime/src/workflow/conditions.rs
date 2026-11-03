//! Private v1 condition contract. The existing comparator owns operand semantics;
//! this plan owns nonempty groups, AND/OR truth and ordered first-match selection.
use std::collections::BTreeMap;

use deepagent_core::error::{CoreError, Result};
use serde_json::{json, Map, Value};

use super::{branches::BranchContract, values};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Logic {
    And,
    Or,
}

impl Logic {
    fn matches(self, comparisons: &[bool]) -> bool {
        match self {
            Self::And => comparisons.iter().all(|value| *value),
            Self::Or => comparisons.iter().any(|value| *value),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ConditionGroup {
    logic: Logic,
    operators: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ConditionPlan {
    branches: BranchContract,
    groups: Vec<ConditionGroup>,
    // Parallel arrays are constructed together only by compile(). Resolving the
    // entire operand namespace once preserves the shared byte/depth budget and
    // never reinterprets inserted upstream strings as template syntax.
    operands: Value,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::invalid(format!("ConditionConfigInvalid: {reason}"))
}

fn aliased<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    alias: &str,
) -> Result<Option<&'a Value>> {
    if object.contains_key(field) && object.contains_key(alias) {
        return Err(invalid("conflicting condition field aliases"));
    }
    Ok(object.get(field).or_else(|| object.get(alias)))
}

impl ConditionPlan {
    pub(super) fn compile(config: &Map<String, Value>) -> Result<Self> {
        let branches = BranchContract::compile(config)?;
        let declarations = config
            .get("conditions")
            .and_then(Value::as_array)
            .filter(|groups| !groups.is_empty())
            .ok_or_else(|| invalid("requires nonempty conditions"))?;
        let mut groups = Vec::new();
        let mut operands = Vec::new();
        for declaration in declarations {
            let group = declaration
                .as_object()
                .ok_or_else(|| invalid("group must be an object"))?;
            let logic = match aliased(group, "logic", "logicalOperator")? {
                None => Logic::And,
                Some(Value::String(value)) if value == "and" => Logic::And,
                Some(Value::String(value)) if value == "or" => Logic::Or,
                _ => return Err(invalid("group logic must be 'and' or 'or'")),
            };
            let items = aliased(group, "items", "conditions")?
                .and_then(Value::as_array)
                .filter(|items| !items.is_empty())
                .ok_or_else(|| invalid("group requires nonempty comparisons"))?;
            let mut operators = Vec::new();
            let mut bindings = Vec::new();
            for item in items {
                let comparison = item
                    .as_object()
                    .ok_or_else(|| invalid("comparison must be an object"))?;
                let operator = comparison
                    .get("operator")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("comparison requires an operator"))?;
                values::validate_operator(operator)
                    .map_err(|_| invalid("unknown comparison operator"))?;
                let left = aliased(comparison, "variable", "left")?
                    .ok_or_else(|| invalid("comparison requires a left operand"))?;
                let right = aliased(comparison, "value", "right")?
                    .ok_or_else(|| invalid("comparison requires a right operand"))?;
                operators.push(operator.to_owned());
                bindings.push(json!([left, right]));
            }
            groups.push(ConditionGroup { logic, operators });
            operands.push(Value::Array(bindings));
        }
        Ok(Self {
            branches,
            groups,
            operands: Value::Array(operands),
        })
    }

    pub(super) fn evaluate(&self, previous: &BTreeMap<String, Value>) -> Result<Value> {
        let operands = values::resolve(&self.operands, previous)?;
        for (index, group) in self.groups.iter().enumerate() {
            // All comparisons in an evaluated group must be valid, even after
            // a decisive result. A type error must not become a boolean choice.
            let comparisons = group
                .operators
                .iter()
                .enumerate()
                .map(|(item, operator)| {
                    values::compare(
                        &operands[index][item][0],
                        operator,
                        &operands[index][item][1],
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            if group.logic.matches(&comparisons) {
                return Ok(json!({"branch":self.branches.cases[index]}));
            }
        }
        Ok(json!({"branch":self.branches.fallback}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(groups: Value) -> Result<ConditionPlan> {
        ConditionPlan::compile(&Map::from_iter([("conditions".into(), groups)]))
    }

    #[test]
    fn h198_truth_has_the_correct_identity_for_one_and_many_comparisons() {
        for values in [
            vec![false],
            vec![true],
            vec![false, true, false],
            vec![true; 5],
        ] {
            assert_eq!(Logic::And.matches(&values), values.iter().all(|v| *v));
            assert_eq!(Logic::Or.matches(&values), values.iter().any(|v| *v));
        }
        for logic in ["and", "or"] {
            for a in [false, true] {
                for b in [false, true] {
                    let p = plan(json!([{"id":"selected","logic":logic,"items":[
                        {"variable":a,"operator":"is","value":true},
                        {"variable":b,"operator":"is","value":true}
                    ]}]))
                    .unwrap();
                    let expected = if logic == "and" { a && b } else { a || b };
                    assert_eq!(
                        p.evaluate(&BTreeMap::new()).unwrap(),
                        json!({"branch":if expected {"true"} else {"false"}})
                    );
                }
            }
        }
    }

    #[test]
    fn h198_admission_rejects_empty_unknown_missing_and_conflicting_aliases() {
        for groups in [
            json!([]),
            json!(null),
            json!([{}]),
            json!([{"id":"x","items":[]}]),
            json!([{"id":"x","logic":"xor","items":[{"variable":1,"operator":"is","value":1}]}]),
            json!([{"id":"x","logic":"and","logicalOperator":"or","items":[{"variable":1,"operator":"is","value":1}]}]),
            json!([{"id":"x","items":[{"variable":1,"operator":"unknown","value":1}]}]),
            json!([{"id":"x","items":[{"operator":"is","value":1}]}]),
            json!([{"id":"x","items":[{"variable":1,"operator":"is"}]}]),
            json!([{"id":"x","items":[{"variable":1,"value":1}]}]),
            json!([{"id":"x","items":[{"variable":1,"left":1,"operator":"is","value":1}]}]),
        ] {
            assert!(plan(groups).is_err());
        }
        let p = plan(json!([{"id":"x","conditions":[{"left":null,"operator":"is","right":null}]}]))
            .unwrap();
        assert_eq!(
            p.evaluate(&BTreeMap::new()).unwrap(),
            json!({"branch":"true"})
        );
    }

    #[test]
    fn h198_eager_group_errors_and_ordered_first_match_do_not_fabricate_truth() {
        for (logic, decisive) in [("or", true), ("and", false)] {
            let p = plan(json!([{"id":"x","logic":logic,"items":[
                {"variable":decisive,"operator":"is","value":true},
                {"variable":{},"operator":"gt","value":"PRIVATE_CANARY"}
            ]}]))
            .unwrap();
            let error = p.evaluate(&BTreeMap::new()).unwrap_err().to_string();
            assert!(!error.contains("PRIVATE_CANARY"));
        }
        let p = plan(json!([
            {"id":"first","items":[{"variable":1,"operator":"is","value":1}]},
            {"id":"second","items":[{"variable":{},"operator":"gt","value":1}]}
        ]))
        .unwrap();
        assert_eq!(
            p.evaluate(&BTreeMap::new()).unwrap(),
            json!({"branch":"first"})
        );
    }

    #[test]
    fn h198_operands_share_one_resolution_budget_and_inserted_text_stays_data() {
        let p = plan(json!([{"id":"x","items":[
            {"variable":"{{#source.a#}}","operator":"is","value":"{{#source.b#}}"}
        ]}]))
        .unwrap();
        let previous =
            |a: Value, b: Value| BTreeMap::from([("source".into(), json!({"a":a,"b":b}))]);
        let text = json!("{{#missing.secret#}}");
        assert_eq!(
            p.evaluate(&previous(text.clone(), text)).unwrap(),
            json!({"branch":"true"})
        );
        let error = p
            .evaluate(&previous(
                json!("\"".repeat(300_000)),
                json!("\"".repeat(300_000)),
            ))
            .unwrap_err()
            .to_string();
        assert!(error.contains("1 MiB"));
        assert!(p.evaluate(&BTreeMap::new()).is_err());
    }
}
