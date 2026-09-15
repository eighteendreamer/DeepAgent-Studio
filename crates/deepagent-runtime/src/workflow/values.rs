use deepagent_core::error::{CoreError, Result};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::{self, Write};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;

fn invalid(context: &str, reason: &str) -> CoreError {
    CoreError::Invalid(format!("workflow {context}: {reason}"))
}

struct Budget {
    used: usize,
    context: &'static str,
}

impl Budget {
    fn new(context: &'static str) -> Self {
        Self { used: 0, context }
    }

    fn add(&mut self, bytes: usize) -> Result<()> {
        if bytes > MAX_BYTES - self.used {
            return Err(invalid(self.context, "serialized value exceeds 1 MiB"));
        }
        self.used += bytes;
        Ok(())
    }
}

impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.add(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "size limit exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn depth_limit(depth: usize, context: &str) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid(context, "value or path exceeds depth 64"));
    }
    Ok(())
}

fn measure(value: &Value, depth: usize, budget: &mut Budget) -> Result<()> {
    depth_limit(depth, budget.context)?;
    match value {
        Value::Array(items) => {
            budget.add(2 + items.len().saturating_sub(1))?;
            for item in items {
                measure(item, depth + 1, budget)?;
            }
        }
        Value::Object(items) => {
            budget.add(2 + items.len().saturating_sub(1))?;
            for (key, item) in items {
                serde_json::to_writer(&mut *budget, key)
                    .map_err(|_| invalid(budget.context, "serialized key exceeds 1 MiB"))?;
                budget.add(1)?;
                measure(item, depth + 1, budget)?;
            }
        }
        _ => serde_json::to_writer(&mut *budget, value)
            .map_err(|_| invalid(budget.context, "serialized value exceeds 1 MiB"))?,
    }
    Ok(())
}

fn check_value(value: &Value, context: &'static str) -> Result<()> {
    measure(value, 0, &mut Budget::new(context))
}

fn check_text(text: &str, context: &str) -> Result<()> {
    if text.len() > MAX_BYTES {
        return Err(invalid(context, "text exceeds 1 MiB"));
    }
    Ok(())
}

fn safe_segment(segment: &str, node: bool) -> bool {
    !segment.is_empty()
        && !matches!(segment, "__proto__" | "constructor" | "prototype")
        && segment
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || (node && c == b'-'))
}

fn validate_path(path: &[String], node: bool) -> Result<()> {
    depth_limit(path.len(), "path")?;
    if path.len() < if node { 2 } else { 1 } {
        return Err(invalid("path", "not enough identifier segments"));
    }
    let mut budget = Budget::new("path");
    for (index, segment) in path.iter().enumerate() {
        budget.add(segment.len() + usize::from(index != 0))?;
        if !safe_segment(segment, node && index == 0) {
            return Err(invalid(
                "path",
                &format!("unsafe or malformed segment {index}"),
            ));
        }
    }
    Ok(())
}

fn parse_path(text: &str, node: bool) -> Result<Vec<String>> {
    check_text(text, "path")?;
    let mut path = Vec::new();
    for segment in text.split('.') {
        depth_limit(path.len() + 1, "path")?;
        path.push(segment.to_owned());
    }
    validate_path(&path, node)?;
    Ok(path)
}

enum TokenKind {
    Reference(Vec<String>),
    Alias(String),
}

struct Token {
    start: usize,
    end: usize,
    kind: TokenKind,
}

fn tokens(text: &str, aliases: bool) -> Result<Vec<Token>> {
    check_text(text, "reference/template")?;
    let mut found = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let rest = &text[cursor..];
        if let Some(body) = rest.strip_prefix("{{#") {
            let close = body
                .find("#}}")
                .ok_or_else(|| invalid("reference", "missing canonical closing marker"))?;
            let path = parse_path(&body[..close], true)?;
            let end = cursor + 3 + close + 3;
            found.push(Token {
                start: cursor,
                end,
                kind: TokenKind::Reference(path),
            });
            cursor = end;
        } else if rest.starts_with("#}}") {
            return Err(invalid("reference", "unexpected canonical closing marker"));
        } else if aliases && rest.starts_with("{{") {
            let close = rest[2..]
                .find("}}")
                .ok_or_else(|| invalid("template", "missing alias closing marker"))?;
            let name = rest[2..2 + close].trim();
            if !safe_segment(name, false)
                || !name
                    .bytes()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            {
                return Err(invalid(
                    "template",
                    "only simple identifier aliases are supported",
                ));
            }
            let end = cursor + 2 + close + 2;
            found.push(Token {
                start: cursor,
                end,
                kind: TokenKind::Alias(name.to_owned()),
            });
            cursor = end;
        } else if aliases
            && ["{%", "{#", "%}", "#}", "}}"]
                .iter()
                .any(|marker| rest.starts_with(marker))
        {
            return Err(invalid(
                "template",
                "unsupported control/comment tag or closing marker",
            ));
        } else {
            cursor += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(found)
}

pub fn references(value: &Value) -> Result<Vec<Vec<String>>> {
    fn visit(value: &Value, found: &mut Vec<Vec<String>>) -> Result<()> {
        match value {
            Value::String(text) => {
                for token in tokens(text, false)? {
                    if let TokenKind::Reference(path) = token.kind {
                        found.push(path);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, found)?;
                }
            }
            Value::Object(items) => {
                for item in items.values() {
                    visit(item, found)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    check_value(value, "references input")?;
    let mut found = Vec::new();
    visit(value, &mut found)?;
    Ok(found)
}

fn traverse<'a>(mut value: &'a Value, path: &[String]) -> Result<Option<&'a Value>> {
    for (index, segment) in path.iter().enumerate() {
        value = match value {
            Value::Object(object) => match object.get(segment) {
                Some(value) => value,
                None => return Ok(None),
            },
            Value::Array(array) => {
                if !segment.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(invalid(
                        "lookup",
                        &format!("array index at segment {index} must be numeric"),
                    ));
                }
                let offset = segment
                    .parse::<usize>()
                    .map_err(|_| invalid("lookup", "array index exceeds supported range"))?;
                match array.get(offset) {
                    Some(value) => value,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
    }
    Ok(Some(value))
}

fn lookup_ref<'a>(
    path: &[String],
    outputs: &'a BTreeMap<String, Value>,
) -> Result<Option<&'a Value>> {
    match outputs.get(&path[0]) {
        Some(value) => traverse(value, &path[1..]),
        None => Ok(None),
    }
}

pub fn lookup(path: &[String], outputs: &BTreeMap<String, Value>) -> Result<Option<Value>> {
    validate_path(path, true)?;
    lookup_ref(path, outputs)?
        .map(|value| {
            check_value(value, "lookup output")?;
            Ok(value.clone())
        })
        .transpose()
}

fn required_ref<'a>(path: &[String], outputs: &'a BTreeMap<String, Value>) -> Result<&'a Value> {
    lookup_ref(path, outputs)?.ok_or_else(|| invalid("reference", "required output is unavailable"))
}

fn append(text: &mut String, piece: &str) -> Result<()> {
    if piece.len() > MAX_BYTES - text.len() {
        return Err(invalid("render output", "text exceeds 1 MiB"));
    }
    text.push_str(piece);
    Ok(())
}

fn append_value(text: &mut String, value: &Value) -> Result<()> {
    check_value(value, "render value")?;
    match value {
        Value::String(piece) => append(text, piece),
        _ => append(
            text,
            &serde_json::to_string(value)
                .map_err(|_| invalid("render value", "cannot serialize JSON"))?,
        ),
    }
}

fn render_tokens(
    text: &str,
    tokens: &[Token],
    bindings: Option<&Map<String, Value>>,
    outputs: &BTreeMap<String, Value>,
) -> Result<String> {
    let mut rendered = String::new();
    let mut cursor = 0;
    for token in tokens {
        append(&mut rendered, &text[cursor..token.start])?;
        let value = match &token.kind {
            TokenKind::Reference(path) => required_ref(path, outputs)?,
            TokenKind::Alias(name) => bindings
                .and_then(|bindings| bindings.get(name))
                .ok_or_else(|| invalid("template", "required alias is unavailable"))?,
        };
        append_value(&mut rendered, value)?;
        cursor = token.end;
    }
    append(&mut rendered, &text[cursor..])?;
    Ok(rendered)
}

fn resolve_inner(
    value: &Value,
    outputs: &BTreeMap<String, Value>,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value> {
    depth_limit(depth, "resolve output")?;
    match value {
        Value::String(text) => {
            let parsed = tokens(text, false)?;
            if let [Token {
                start: 0,
                end,
                kind: TokenKind::Reference(path),
            }] = parsed.as_slice()
            {
                if *end == text.len() {
                    let value = required_ref(path, outputs)?;
                    measure(value, depth, budget)?;
                    return Ok(value.clone());
                }
            }
            let value = Value::String(render_tokens(text, &parsed, None, outputs)?);
            measure(&value, depth, budget)?;
            Ok(value)
        }
        Value::Array(items) => {
            budget.add(2 + items.len().saturating_sub(1))?;
            items
                .iter()
                .map(|item| resolve_inner(item, outputs, depth + 1, budget))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array)
        }
        Value::Object(items) => {
            budget.add(2 + items.len().saturating_sub(1))?;
            let mut object = Map::new();
            for (key, item) in items {
                serde_json::to_writer(&mut *budget, key)
                    .map_err(|_| invalid("resolve output", "serialized key exceeds 1 MiB"))?;
                budget.add(1)?;
                object.insert(
                    key.clone(),
                    resolve_inner(item, outputs, depth + 1, budget)?,
                );
            }
            Ok(Value::Object(object))
        }
        _ => {
            measure(value, depth, budget)?;
            Ok(value.clone())
        }
    }
}

pub fn resolve(value: &Value, outputs: &BTreeMap<String, Value>) -> Result<Value> {
    check_value(value, "resolve input")?;
    resolve_inner(value, outputs, 0, &mut Budget::new("resolve output"))
}

pub fn render_template(
    template: &str,
    bindings: &Map<String, Value>,
    outputs: &BTreeMap<String, Value>,
) -> Result<String> {
    let parsed = tokens(template, true)?;
    render_tokens(template, &parsed, Some(bindings), outputs)
}

#[derive(Clone, Copy)]
enum Numeric {
    Integer(i128),
    Float(f64),
}

fn numeric(value: &Value, text_allowed: bool) -> Result<Numeric> {
    let number = match value {
        Value::Number(number) => number.clone(),
        Value::String(text) if text_allowed => text
            .trim()
            .parse::<serde_json::Number>()
            .map_err(|_| invalid("numeric operand", "expected finite numeric text"))?,
        _ => return Err(invalid("numeric operand", "expected a number")),
    };
    if let Some(integer) = number.as_i64() {
        return Ok(Numeric::Integer(i128::from(integer)));
    }
    if let Some(integer) = number.as_u64() {
        return Ok(Numeric::Integer(i128::from(integer)));
    }
    let float = number
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| invalid("numeric operand", "expected a finite number"))?;
    Ok(Numeric::Float(float))
}

impl Numeric {
    fn float(self) -> f64 {
        match self {
            Self::Integer(integer) => integer as f64,
            Self::Float(float) => float,
        }
    }

    fn compare(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) => left.cmp(&right),
            (Self::Integer(integer), Self::Float(float)) => {
                if float >= 18_446_744_073_709_551_616.0 {
                    Ordering::Less
                } else if float < -9_223_372_036_854_775_808.0 {
                    Ordering::Greater
                } else {
                    integer.cmp(&(float as i128)).then_with(|| {
                        0.0_f64
                            .partial_cmp(&float.fract())
                            .unwrap_or(Ordering::Equal)
                    })
                }
            }
            (Self::Float(_), Self::Integer(_)) => other.compare(self).reverse(),
            (Self::Float(left), Self::Float(right)) => {
                left.partial_cmp(&right).unwrap_or(Ordering::Equal)
            }
        }
    }
}

fn validate_operator(operator: &str) -> Result<()> {
    match operator {
        "is" | "is-not" | "contains" | "not-contains" | "starts-with" | "ends-with" | "empty"
        | "not-empty" | "gt" | "gte" | "lt" | "lte" => Ok(()),
        _ => Err(invalid("compare", "unknown operator")),
    }
}

fn compare_inner(left: &Value, operator: &str, right: &Value) -> Result<bool> {
    match operator {
        "is" | "is-not" => {
            let equal = match (left, right) {
                (Value::Number(_), _) => {
                    numeric(left, false)?.compare(numeric(right, true)?) == Ordering::Equal
                }
                (Value::Null, Value::Null)
                | (Value::Bool(_), Value::Bool(_))
                | (Value::String(_), Value::String(_))
                | (Value::Array(_), Value::Array(_))
                | (Value::Object(_), Value::Object(_)) => left == right,
                _ => return Err(invalid("compare equality", "incompatible operand types")),
            };
            Ok(if operator == "is" { equal } else { !equal })
        }
        "contains" | "not-contains" => {
            let contains = match (left, right) {
                (Value::String(left), Value::String(right)) => left.contains(right.as_str()),
                (Value::Array(items), _) => items.contains(right),
                _ => {
                    return Err(invalid(
                        "compare contains",
                        "expected string/string or array/item",
                    ))
                }
            };
            Ok(if operator == "contains" {
                contains
            } else {
                !contains
            })
        }
        "starts-with" | "ends-with" => match (left, right) {
            (Value::String(left), Value::String(right)) => Ok(if operator == "starts-with" {
                left.starts_with(right.as_str())
            } else {
                left.ends_with(right.as_str())
            }),
            _ => Err(invalid("compare prefix/suffix", "expected two strings")),
        },
        "empty" | "not-empty" => {
            let empty = match left {
                Value::Null => true,
                Value::String(text) => text.is_empty(),
                Value::Array(items) => items.is_empty(),
                Value::Object(items) => items.is_empty(),
                _ => {
                    return Err(invalid(
                        "compare empty",
                        "expected null, string, array or object",
                    ))
                }
            };
            Ok(if operator == "empty" { empty } else { !empty })
        }
        "gt" | "gte" | "lt" | "lte" => {
            let order = numeric(left, false)?.compare(numeric(right, true)?);
            Ok(match operator {
                "gt" => order.is_gt(),
                "gte" => !order.is_lt(),
                "lt" => order.is_lt(),
                _ => !order.is_gt(),
            })
        }
        _ => Err(invalid("compare", "unknown operator")),
    }
}

pub fn compare(left: &Value, operator: &str, right: &Value) -> Result<bool> {
    check_value(left, "compare left")?;
    check_value(right, "compare right")?;
    compare_inner(left, operator, right)
}

fn integer_value(integer: i128) -> Result<Value> {
    if let Ok(integer) = i64::try_from(integer) {
        Ok(Value::from(integer))
    } else if let Ok(integer) = u64::try_from(integer) {
        Ok(Value::from(integer))
    } else {
        Err(invalid(
            "assignment",
            "integer arithmetic exceeds JSON integer range",
        ))
    }
}

fn arithmetic(current: &Value, mode: &str, value: &Value) -> Result<Value> {
    let left = numeric(current, false)?;
    let right = numeric(value, true)?;
    if mode == "divide" && right.float() == 0.0 {
        return Err(invalid("assignment divide", "division by zero"));
    }
    if let (Numeric::Integer(left), Numeric::Integer(right)) = (left, right) {
        let integer = match mode {
            "increment" => Some(left.checked_add(right)),
            "decrement" => Some(left.checked_sub(right)),
            "multiply" => Some(left.checked_mul(right)),
            "divide" if left % right == 0 => Some(left.checked_div(right)),
            _ => None,
        };
        if let Some(integer) = integer {
            return integer_value(
                integer.ok_or_else(|| invalid("assignment", "integer arithmetic overflow"))?,
            );
        }
    }
    let result = match mode {
        "increment" => left.float() + right.float(),
        "decrement" => left.float() - right.float(),
        "multiply" => left.float() * right.float(),
        "divide" => left.float() / right.float(),
        _ => return Err(invalid("assignment", "unknown arithmetic mode")),
    };
    serde_json::Number::from_f64(result)
        .map(Value::Number)
        .ok_or_else(|| invalid("assignment", "arithmetic result is not finite"))
}

pub fn assign_value(current: &Value, mode: &str, value: &Value) -> Result<Value> {
    check_value(current, "assignment current")?;
    check_value(value, "assignment value")?;
    match mode {
        "set" => Ok(value.clone()),
        "increment" | "decrement" | "multiply" | "divide" => arithmetic(current, mode, value),
        "clear" => Ok(match current {
            Value::String(_) => Value::String(String::new()),
            Value::Array(_) => Value::Array(Vec::new()),
            Value::Object(_) => Value::Object(Map::new()),
            _ => Value::Null,
        }),
        "remove-first" | "remove-last" => {
            let items = current
                .as_array()
                .ok_or_else(|| invalid("assignment removal", "expected an array"))?;
            let remaining = if mode == "remove-first" {
                &items[usize::from(!items.is_empty())..]
            } else {
                &items[..items.len().saturating_sub(1)]
            };
            Ok(Value::Array(remaining.to_vec()))
        }
        _ => Err(invalid("assignment", "unknown mode")),
    }
}

struct Predicate {
    path: Vec<String>,
    operator: String,
    value: Value,
}

fn optional_field(field: &str) -> Result<Vec<String>> {
    if field.is_empty() {
        Ok(Vec::new())
    } else {
        parse_path(field, false)
    }
}

fn predicate(condition: &str) -> Result<Predicate> {
    check_text(condition, "list filter condition")?;
    let parsed: Value = serde_json::from_str(condition).map_err(|_| {
        invalid(
            "list filter",
            "condition must be a JSON object with field/operator/value",
        )
    })?;
    check_value(&parsed, "list filter condition")?;
    let object = parsed
        .as_object()
        .ok_or_else(|| invalid("list filter", "condition must be a JSON object"))?;
    if object.len() != 3 || !object.contains_key("value") {
        return Err(invalid(
            "list filter",
            "expected exactly field/operator/value settings",
        ));
    }
    let field = object
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("list filter", "field must be a dot-path string"))?;
    let operator = object
        .get("operator")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("list filter", "operator must be a string"))?;
    validate_operator(operator)?;
    Ok(Predicate {
        path: optional_field(field)?,
        operator: operator.to_owned(),
        value: object["value"].clone(),
    })
}

fn item_field<'a>(item: &'a Value, path: &[String]) -> Result<&'a Value> {
    traverse(item, path)?.ok_or_else(|| invalid("list item", "required field is unavailable"))
}

enum SortKey<'a> {
    Number(Numeric),
    String(&'a str),
}

impl<'a> SortKey<'a> {
    fn new(value: &'a Value) -> Result<Self> {
        match value {
            Value::Number(_) => Ok(Self::Number(numeric(value, false)?)),
            Value::String(text) => Ok(Self::String(text)),
            _ => Err(invalid("list sort", "keys must be numbers or strings")),
        }
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left.compare(*right),
            (Self::String(left), Self::String(right)) => left.cmp(right),
            _ => Ordering::Equal,
        }
    }
}

fn sorted<'a>(items: &'a [Value], field: &str, order_by: &str) -> Result<Vec<&'a Value>> {
    // The UI supplies an order_by path; explicit field plus asc/desc is also accepted.
    let (path, descending) = match order_by {
        "" | "asc" => (optional_field(field)?, false),
        "desc" => (optional_field(field)?, true),
        _ if field.is_empty() => (parse_path(order_by, false)?, false),
        _ => {
            return Err(invalid(
                "list sort",
                "order_by must be asc/desc when field is set",
            ))
        }
    };
    let mut keyed = Vec::with_capacity(items.len());
    for item in items {
        let key = SortKey::new(item_field(item, &path)?)?;
        if let Some((first, _)) = keyed.first() {
            if std::mem::discriminant(first) != std::mem::discriminant(&key) {
                return Err(invalid(
                    "list sort",
                    "mixed number/string keys are unsupported",
                ));
            }
        }
        keyed.push((key, item));
    }
    keyed.sort_by(|(left, _), (right, _)| {
        let order = left.compare(right);
        if descending {
            order.reverse()
        } else {
            order
        }
    });
    Ok(keyed.into_iter().map(|(_, item)| item).collect())
}

pub fn operate_list(
    input: &Value,
    action: &str,
    condition: &str,
    field: &str,
    order_by: &str,
    limit: usize,
) -> Result<Value> {
    check_value(input, "list input")?;
    check_text(condition, "list condition")?;
    check_text(field, "list field")?;
    check_text(order_by, "list order_by")?;
    let items = input
        .as_array()
        .ok_or_else(|| invalid("list input", "expected an array"))?;
    if (action != "filter" && !condition.is_empty())
        || (!matches!(action, "map" | "sort") && !field.is_empty())
        || (action != "sort" && !order_by.is_empty())
    {
        return Err(invalid(
            "list settings",
            "setting is unsupported for the selected action",
        ));
    }
    let mut result = match action {
        "filter" => {
            let predicate = predicate(condition)?;
            let mut result = Vec::new();
            for item in items {
                if compare_inner(
                    item_field(item, &predicate.path)?,
                    &predicate.operator,
                    &predicate.value,
                )? {
                    result.push(item);
                }
            }
            result
        }
        "map" => {
            let path = parse_path(field, false)?;
            items
                .iter()
                .map(|item| item_field(item, &path))
                .collect::<Result<Vec<_>>>()?
        }
        "sort" => sorted(items, field, order_by)?,
        "limit" => items.iter().collect(),
        _ => return Err(invalid("list action", "unknown action")),
    };
    if limit != 0 {
        result.truncate(limit);
    }
    let first = result.first().copied().unwrap_or(&Value::Null);
    let last = result.last().copied().unwrap_or(&Value::Null);
    let mut budget = Budget::new("list output");
    budget.add(b"{\"result\":[],\"first\":,\"last\":}".len() + result.len().saturating_sub(1))?;
    for item in &result {
        measure(item, 2, &mut budget)?;
    }
    measure(first, 1, &mut budget)?;
    measure(last, 1, &mut budget)?;
    let mut output = Map::new();
    output.insert("first".to_owned(), first.clone());
    output.insert("last".to_owned(), last.clone());
    output.insert(
        "result".to_owned(),
        Value::Array(result.into_iter().cloned().collect()),
    );
    Ok(Value::Object(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn outputs() -> BTreeMap<String, Value> {
        BTreeMap::from([(
            "node-1".to_owned(),
            json!({
                "text": "你好🌍", "number": 3, "null": null,
                "flag": false, "items": [{"score": 12}],
                "object": {"key": "value"}, "literal": "{{#missing.output#}}"
            }),
        )])
    }

    fn path(text: &str) -> Vec<String> {
        text.split('.').map(str::to_owned).collect()
    }

    #[test]
    fn preserves_reference_types_and_literal_whitespace() {
        let outputs = outputs();
        for name in ["text", "number", "null", "flag", "items", "object"] {
            assert_eq!(
                resolve(&json!(format!("{{{{#node-1.{name}#}}}}")), &outputs).unwrap(),
                outputs["node-1"][name]
            );
        }
        assert_eq!(resolve(&json!("  {{#node-1.number#}}\n{{#node-1.object#}} / {{#node-1.flag#}} {{#node-1.null#}}  "), &outputs).unwrap(), json!("  3\n{\"key\":\"value\"} / false null  "));
        assert_eq!(
            resolve(&json!("{{#node-1.literal#}}"), &outputs).unwrap(),
            json!("{{#missing.output#}}")
        );
    }

    #[test]
    fn nested_references_ignore_object_keys_and_aliases() {
        let value =
            json!({"{{#missing.key#}}": ["{{alias}}", {"x": "{{#node-1.items.0.score#}}"}]});
        assert_eq!(
            references(&value).unwrap(),
            vec![path("node-1.items.0.score")]
        );
        assert_eq!(
            resolve(&value, &outputs()).unwrap(),
            json!({"{{#missing.key#}}": ["{{alias}}", {"x": 12}]})
        );
        assert_eq!(
            references(&json!("{{#node-1.number#}}{{#node-1.number#}}"))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn missing_skipped_and_null_are_distinct_for_branch_aggregation() {
        let outputs = outputs();
        assert_eq!(lookup(&path("skipped.output"), &outputs).unwrap(), None);
        assert_eq!(lookup(&path("node-1.missing"), &outputs).unwrap(), None);
        assert_eq!(
            lookup(&path("node-1.null"), &outputs).unwrap(),
            Some(Value::Null)
        );
        assert_eq!(lookup(&path("node-1.null.field"), &outputs).unwrap(), None);
        assert_eq!(lookup(&path("node-1.items.9"), &outputs).unwrap(), None);
        let selected = ["skipped.output", "node-1.null", "node-1.number"]
            .into_iter()
            .find_map(|reference| lookup(&path(reference), &outputs).unwrap());
        assert_eq!(selected, Some(Value::Null));
        assert!(resolve(&json!("required {{#skipped.output#}}"), &outputs).is_err());
    }

    #[test]
    fn rejects_unsafe_malformed_and_excessive_paths() {
        for text in [
            "{{#node#}}",
            "{{#node.x}}",
            "{{#node.x#}",
            "#}}",
            "{{#node..x#}}",
            "{{#node.x-y#}}",
            "{{#node.中文#}}",
            "{{#node.x[0]#}}",
            "{{#node.x#}}{{#broken",
            "{{#node. x#}}",
            "{{#node.x{{#other.x#}}",
        ] {
            assert!(references(&json!(text)).is_err(), "{text}");
            assert!(resolve(&json!(text), &outputs()).is_err(), "{text}");
        }
        for segment in ["__proto__", "constructor", "prototype"] {
            assert!(lookup(&path(&format!("node-1.{segment}")), &outputs()).is_err());
            assert!(references(&json!(format!("{{{{#{segment}.x#}}}}"))).is_err());
        }
        assert!(lookup(&[], &outputs()).is_err());
        assert!(lookup(&path("node-1.items.first"), &outputs()).is_err());
        assert!(lookup(&path("node-1.items.9999999999999999999999999"), &outputs()).is_err());
        assert!(lookup(&vec!["x".to_owned(); MAX_DEPTH + 1], &outputs()).is_err());
        assert_eq!(
            lookup(&path("node-1.items.0.score"), &outputs()).unwrap(),
            Some(json!(12))
        );
    }

    #[test]
    fn unicode_is_preserved_and_never_sliced_at_invalid_boundaries() {
        let text = "前缀é🦀 {{#node-1.text#}} 后缀";
        assert_eq!(
            resolve(&json!(text), &outputs()).unwrap(),
            json!("前缀é🦀 你好🌍 后缀")
        );
        for text in [
            "中{文",
            "é{{别名}}",
            "🦀",
            "\u{0301}",
            "中文{{#node-1.text#}}é",
        ] {
            assert!(references(&json!(text)).is_ok());
        }
    }

    #[test]
    fn template_supports_aliases_and_references_without_evaluation() {
        let bindings = Map::from_iter([
            ("name".to_owned(), json!("<b>{{unresolved}}</b>")),
            ("data".to_owned(), json!([1, true])),
            ("nothing".to_owned(), Value::Null),
        ]);
        assert_eq!(
            render_template(
                " \n{{ name }} {{\tdata\n}} {{#node-1.number#}} {{nothing}} ",
                &bindings,
                &outputs()
            )
            .unwrap(),
            " \n<b>{{unresolved}}</b> [1,true] 3 null "
        );
        for template in [
            "{{missing}}",
            "{{ name | upper }}",
            "{{ data.0 }}",
            "{{ name() }}",
            "{{ 1 + 2 }}",
            "{% if name %}x{% endif %}",
            "{# comment #}",
            "{{#comment}}",
            "{{ name",
            "{{constructor}}",
            "{{/if}}",
            "}}",
            "{{#node-1.text#}}#}",
        ] {
            assert!(
                render_template(template, &bindings, &outputs()).is_err(),
                "{template}"
            );
        }
    }

    #[test]
    fn numeric_predicates_coerce_only_numeric_rhs_text() {
        for (operator, right, expected) in [
            ("is", "3", true),
            ("is-not", "3.0", false),
            ("gt", "2", true),
            ("gte", "3", true),
            ("lt", "4", true),
            ("lte", "2", false),
        ] {
            assert_eq!(
                compare(&json!(3), operator, &json!(right)).unwrap(),
                expected
            );
        }
        for right in ["NaN", "Infinity", "1e999", "secret-invalid-number"] {
            let error = compare(&json!(3), "gt", &json!(right))
                .unwrap_err()
                .to_string();
            assert!(!error.contains(right));
        }
        assert!(compare(&json!(true), "is", &json!("true")).is_err());
        assert!(compare(&json!({}), "is", &json!("{}")).is_err());
        assert!(compare(&json!("3"), "gt", &json!(2)).is_err());
        assert!(compare(&json!(3), "unknown", &json!(3)).is_err());
        assert!(compare(&json!({"x": true}), "is", &json!({"x": true})).unwrap());
        assert!(!compare(&json!({"x": true}), "is", &json!({"x": "true"})).unwrap());
        assert!(compare(
            &json!(9_007_199_254_740_993_u64),
            "gt",
            &json!(9_007_199_254_740_992_u64)
        )
        .unwrap());
        assert!(compare(&json!(u64::MAX), "is", &json!(u64::MAX.to_string())).unwrap());
        assert!(compare(&json!(u64::MAX), "lt", &json!(18_446_744_073_709_551_616.0)).unwrap());
        assert!(compare(&json!(0), "gt", &json!(-0.5)).unwrap());
        assert!(compare(&json!(-0.0), "is", &json!(0.0)).unwrap());
    }

    #[test]
    fn predicates_validate_containment_and_empty_types() {
        assert!(compare(&json!("中文abc"), "contains", &json!("文a")).unwrap());
        assert!(compare(&json!([{"a": 1}]), "contains", &json!({"a": 1})).unwrap());
        assert!(compare(&json!([1]), "not-contains", &json!("1")).unwrap());
        assert!(compare(&json!("abc"), "starts-with", &json!("a")).unwrap());
        assert!(compare(&json!("abc"), "ends-with", &json!("c")).unwrap());
        for value in [Value::Null, json!(""), json!([]), json!({})] {
            assert!(compare(&value, "empty", &Value::Null).unwrap());
            assert!(!compare(&value, "not-empty", &Value::Null).unwrap());
        }
        assert!(compare(&json!(" "), "not-empty", &Value::Null).unwrap());
        assert!(compare(&json!(false), "empty", &Value::Null).is_err());
        assert!(compare(&json!("x"), "contains", &json!(3)).is_err());
        assert!(compare(&json!(3), "starts-with", &json!("3")).is_err());
    }

    #[test]
    fn assignment_is_pure_and_arithmetic_is_finite() {
        let original = json!([1, 2, 3]);
        assert_eq!(
            assign_value(&original, "set", &json!({"x": 1})).unwrap(),
            json!({"x": 1})
        );
        assert_eq!(
            assign_value(&original, "remove-first", &Value::Null).unwrap(),
            json!([2, 3])
        );
        assert_eq!(
            assign_value(&original, "remove-last", &Value::Null).unwrap(),
            json!([1, 2])
        );
        assert_eq!(original, json!([1, 2, 3]));
        for mode in ["remove-first", "remove-last"] {
            assert_eq!(
                assign_value(&json!([]), mode, &Value::Null).unwrap(),
                json!([])
            );
            assert!(assign_value(&Value::Null, mode, &Value::Null).is_err());
        }
        for (mode, result) in [
            ("increment", 12),
            ("decrement", 4),
            ("multiply", 32),
            ("divide", 2),
        ] {
            assert_eq!(
                assign_value(&json!(8), mode, &json!("4")).unwrap(),
                json!(result)
            );
        }
        assert_eq!(
            assign_value(&json!(3), "divide", &json!(2)).unwrap(),
            json!(1.5)
        );
        assert!(assign_value(&json!(1), "divide", &json!(0)).is_err());
        assert!(assign_value(&json!(1), "divide", &json!(-0.0)).is_err());
        assert!(assign_value(&json!(1e308), "multiply", &json!(10)).is_err());
        assert!(assign_value(&json!(u64::MAX), "increment", &json!(1)).is_err());
        assert!(assign_value(&json!(true), "increment", &json!(1)).is_err());
        assert!(assign_value(&json!(1), "increment", &json!("NaN")).is_err());
        assert!(assign_value(&json!(1), "append", &json!(1)).is_err());
        for (current, empty) in [
            (json!("x"), json!("")),
            (json!([1]), json!([])),
            (json!({"x": 1}), json!({})),
            (json!(3), Value::Null),
        ] {
            assert_eq!(
                assign_value(&current, "clear", &Value::Null).unwrap(),
                empty
            );
        }
    }

    #[test]
    fn list_filter_map_sort_and_limit_match_schema_results() {
        let items = json!([{"score": 10, "nested": ["b"]}, {"score": 2, "nested": ["a"]}, {"score": 3, "nested": ["c"]}]);
        let filtered = operate_list(
            &items,
            "filter",
            r#"{"field":"score","operator":"gte","value":3}"#,
            "",
            "",
            0,
        )
        .unwrap();
        assert_eq!(filtered["result"], json!([items[0], items[2]]));
        assert_eq!(filtered["first"], items[0]);
        assert_eq!(filtered["last"], items[2]);
        assert_eq!(
            operate_list(&items, "map", "", "nested.0", "", 2).unwrap(),
            json!({"result": ["b", "a"], "first": "b", "last": "a"})
        );
        assert_eq!(
            operate_list(&items, "sort", "", "", "score", 0).unwrap()["result"],
            json!([items[1], items[2], items[0]])
        );
        assert_eq!(
            operate_list(&items, "sort", "", "score", "desc", 1).unwrap()["first"],
            items[0]
        );
        assert_eq!(
            operate_list(&items, "limit", "", "", "", 1).unwrap()["result"],
            json!([items[0]])
        );
        assert_eq!(
            operate_list(&items, "limit", "", "", "", 0).unwrap()["result"],
            items
        );
        assert_eq!(
            operate_list(&json!([]), "limit", "", "", "", 0).unwrap(),
            json!({"result": [], "first": null, "last": null})
        );
    }

    #[test]
    fn list_sort_is_numeric_lexical_and_stable() {
        assert_eq!(
            operate_list(&json!([10, 2, -1, 2.5]), "sort", "", "", "", 0).unwrap()["result"],
            json!([-1, 2, 2.5, 10])
        );
        assert_eq!(
            operate_list(&json!(["b", "aa", "a"]), "sort", "", "", "asc", 0).unwrap()["result"],
            json!(["a", "aa", "b"])
        );
        let stable = json!([{"k": 2, "id": "a"}, {"k": 1, "id": "b"}, {"k": 2, "id": "c"}]);
        assert_eq!(
            operate_list(&stable, "sort", "", "k", "desc", 0).unwrap()["result"],
            json!([stable[0], stable[2], stable[1]])
        );
        let large = json!([9_007_199_254_740_993_u64, 9_007_199_254_740_992_u64]);
        assert_eq!(
            operate_list(&large, "sort", "", "", "", 0).unwrap()["result"],
            json!([large[1], large[0]])
        );
    }

    #[test]
    fn list_rejects_missing_fields_mixed_types_and_unknown_settings() {
        for condition in [
            "",
            "item.score > 3",
            "[]",
            r#"{"field":"score","operator":"wat","value":3}"#,
            r#"{"field":"score","operator":"gte","value":3,"extra":true}"#,
            r#"{"field":"__proto__","operator":"is","value":null}"#,
        ] {
            assert!(
                operate_list(&json!([]), "filter", condition, "", "", 0).is_err(),
                "{condition}"
            );
        }
        assert!(operate_list(&json!({}), "limit", "", "", "", 0).is_err());
        assert!(operate_list(&json!([]), "eval", "", "", "", 0).is_err());
        assert!(operate_list(&json!([{}]), "map", "", "missing", "", 0).is_err());
        assert!(operate_list(&json!([]), "map", "", "", "", 0).is_err());
        assert!(operate_list(&json!([]), "sort", "", "score", "sideways", 0).is_err());
        assert!(operate_list(&json!([]), "sort", "", "", "score | reverse", 0).is_err());
        assert!(operate_list(&json!([1, "2"]), "sort", "", "", "", 0).is_err());
        assert!(operate_list(&json!([null]), "sort", "", "", "", 0).is_err());
        assert!(operate_list(&json!([{}]), "sort", "", "", "score", 0).is_err());
        assert!(operate_list(&json!([]), "limit", "ignored", "", "", 0).is_err());
        assert!(operate_list(&json!([]), "limit", "", "ignored", "", 0).is_err());
        assert!(operate_list(&json!([]), "map", "", "score", "desc", 0).is_err());
        assert!(operate_list(
            &json!([{}]),
            "filter",
            r#"{"field":"missing","operator":"empty","value":null}"#,
            "",
            "",
            0
        )
        .is_err());
    }

    #[test]
    fn input_bounds_use_serialized_bytes_and_depth() {
        let exact = json!("x".repeat(MAX_BYTES - 2));
        assert!(references(&exact).is_ok());
        assert!(references(&json!("x".repeat(MAX_BYTES - 1))).is_err());
        assert!(references(&json!("\n".repeat(MAX_BYTES / 2))).is_err());
        assert!(references(&json!({"x".repeat(MAX_BYTES): 1})).is_err());
        let mut nested = Value::Null;
        for _ in 0..MAX_DEPTH {
            nested = json!([nested]);
        }
        assert!(references(&nested).is_ok());
        assert!(references(&json!([nested])).is_err());
    }

    #[test]
    fn expansion_and_list_envelopes_are_bounded() {
        let outputs = BTreeMap::from([(
            "node".to_owned(),
            json!({"text": "x".repeat(MAX_BYTES / 2)}),
        )]);
        assert!(render_template("{{#node.text#}}{{#node.text#}}", &Map::new(), &outputs).is_ok());
        assert!(render_template("{{#node.text#}}{{#node.text#}}x", &Map::new(), &outputs).is_err());
        assert!(resolve(&json!("{{#node.text#}}{{#node.text#}}"), &outputs).is_err());
        assert!(resolve(&json!(["{{#node.text#}}", "{{#node.text#}}"]), &outputs).is_err());
        assert!(operate_list(&json!(["x".repeat(MAX_BYTES / 2)]), "limit", "", "", "", 0).is_err());
        let mut nested = Value::Null;
        for _ in 0..MAX_DEPTH {
            nested = json!([nested]);
        }
        let outputs = BTreeMap::from([("node".to_owned(), json!({"deep": nested}))]);
        assert!(resolve(&json!("{{#node.deep#}}"), &outputs).is_ok());
        assert!(resolve(&json!(["{{#node.deep#}}"]), &outputs).is_err());
    }
}
