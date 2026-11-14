//! Bounded `JSONPath` selection with Cardigann :has/:not/:contains predicates.
use super::{EngineError, JsonValue};
const MAX_STEPS: usize = 128;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 32;
#[derive(Debug)]
enum Step {
    Key(String),
    Index(isize),
    Wildcard,
    Union(Vec<Step>),
    Slice(Option<isize>, Option<isize>, isize),
    Recursive(String),
    Predicate(Predicate),
}
#[derive(Debug)]
enum Predicate {
    Has(Vec<Step>),
    Not(Vec<Step>),
    Contains(String),
    Equal(Vec<Step>, JsonValue, bool),
}
fn invalid(reason: &'static str) -> EngineError {
    EngineError::JsonSelector(reason)
}

pub(super) fn select<'a>(
    value: &'a JsonValue,
    path: &str,
) -> Result<Vec<&'a JsonValue>, EngineError> {
    let steps = parse(path, 0)?;
    let mut budget = MAX_NODES;
    evaluate(vec![value], &steps, &mut budget)
}
fn spend(budget: &mut usize) -> Result<(), EngineError> {
    *budget = budget
        .checked_sub(1)
        .ok_or_else(|| invalid("node budget exceeded"))?;
    Ok(())
}
fn evaluate<'a>(
    mut nodes: Vec<&'a JsonValue>,
    steps: &[Step],
    budget: &mut usize,
) -> Result<Vec<&'a JsonValue>, EngineError> {
    for step in steps {
        let mut next = Vec::new();
        for node in nodes {
            visit(node, step, &mut next, budget)?;
        }
        nodes = next;
    }
    Ok(nodes)
}
fn index(i: isize, len: usize) -> Option<usize> {
    let len = isize::try_from(len).ok()?;
    usize::try_from(if i < 0 { len.checked_add(i)? } else { i }).ok()
}
fn visit<'a>(
    node: &'a JsonValue,
    step: &Step,
    output: &mut Vec<&'a JsonValue>,
    budget: &mut usize,
) -> Result<(), EngineError> {
    spend(budget)?;
    match step {
        Step::Key(k) => {
            if let Some(value) = node.get(k) {
                output.push(value);
            }
        }
        Step::Index(i) => {
            if let Some(array) = node.as_array()
                && let Some(value) = index(*i, array.len()).and_then(|i| array.get(i))
            {
                output.push(value);
            }
        }
        Step::Wildcard => match node {
            JsonValue::Array(v) => {
                for v in v {
                    spend(budget)?;
                    output.push(v);
                }
            }
            JsonValue::Object(v) => {
                for v in v.values() {
                    spend(budget)?;
                    output.push(v);
                }
            }
            _ => {}
        },
        Step::Union(steps) => {
            for step in steps {
                visit(node, step, output, budget)?;
            }
        }
        Step::Slice(start, end, stride) => {
            if let Some(array) = node.as_array() {
                let len = isize::try_from(array.len()).map_err(|_| invalid("array too large"))?;
                let bound = |v: isize| if v < 0 { (len + v).max(-1) } else { v.min(len) };
                let mut i = start.map_or(if *stride > 0 { 0 } else { len - 1 }, bound);
                let boundary = end.map_or(if *stride > 0 { len } else { -1 }, bound);
                if *stride > 0 {
                    i = i.max(0);
                }
                while if *stride > 0 {
                    i < boundary
                } else {
                    i > boundary
                } {
                    spend(budget)?;
                    if let Some(v) = usize::try_from(i).ok().and_then(|i| array.get(i)) {
                        output.push(v);
                    }
                    i = i
                        .checked_add(*stride)
                        .ok_or_else(|| invalid("slice overflow"))?;
                }
            }
        }
        Step::Recursive(key) => {
            let mut pending = vec![(node, 0)];
            while let Some((v, depth)) = pending.pop() {
                spend(budget)?;
                if depth > MAX_DEPTH {
                    return Err(invalid("recursive descent depth exceeded"));
                }
                if let Some(obj) = v.as_object() {
                    for (k, v) in obj {
                        spend(budget)?;
                        if key == "*" || key == k {
                            output.push(v);
                        }
                        pending.push((v, depth + 1));
                    }
                } else if let Some(array) = v.as_array() {
                    for v in array {
                        spend(budget)?;
                        pending.push((v, depth + 1));
                    }
                }
            }
        }
        Step::Predicate(predicate) => {
            if matches_predicate(node, predicate, budget)? {
                output.push(node);
            }
        }
    }
    if output.len() > MAX_NODES {
        return Err(invalid("result budget exceeded"));
    }
    Ok(())
}
fn matches_predicate(
    node: &JsonValue,
    pred: &Predicate,
    budget: &mut usize,
) -> Result<bool, EngineError> {
    match pred {
        Predicate::Has(path) => Ok(!evaluate(vec![node], path, budget)?.is_empty()),
        Predicate::Not(path) => Ok(evaluate(vec![node], path, budget)?.is_empty()),
        Predicate::Contains(needle) => Ok(super::extract::json_string(node).contains(needle)),
        Predicate::Equal(path, value, negated) => Ok(evaluate(vec![node], path, budget)?
            .iter()
            .any(|v| (*v == value) != *negated)),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "One bounded parser state machine keeps syntax transitions auditable."
)]
fn parse(path: &str, depth: usize) -> Result<Vec<Step>, EngineError> {
    if depth > MAX_DEPTH || path.len() > 8192 {
        return Err(invalid("selector complexity limit exceeded"));
    }
    let mut rest = path.trim().strip_prefix('$').unwrap_or(path.trim());
    if rest.starts_with('@') {
        rest = &rest[1..];
    }
    let mut steps = Vec::new();
    while !rest.is_empty() {
        if steps.len() >= MAX_STEPS {
            return Err(invalid("too many path steps"));
        }
        if rest.starts_with(':') {
            let open = rest
                .find('(')
                .ok_or_else(|| invalid("malformed predicate"))?;
            let close = closing(rest, open, '(', ')')?;
            let content = rest[open + 1..close].trim();
            steps.push(Step::Predicate(match &rest[1..open] {
                "has" => Predicate::Has(parse(content, depth + 1)?),
                "not" => Predicate::Not(parse(content, depth + 1)?),
                "contains" => Predicate::Contains(unquote(content)?),
                _ => return Err(invalid("unsupported predicate")),
            }));
            rest = &rest[close + 1..];
            continue;
        }
        if let Some(tail) = rest.strip_prefix("..") {
            let end = tail.find(['.', '[', ':']).unwrap_or(tail.len());
            if end == 0 {
                return Err(invalid("recursive descent requires a key"));
            }
            steps.push(Step::Recursive(tail[..end].to_owned()));
            rest = &tail[end..];
            continue;
        }
        if let Some(tail) = rest.strip_prefix('.') {
            rest = tail;
            if rest.is_empty() {
                return Err(invalid("trailing dot"));
            }
        }
        if rest.starts_with('[') {
            let close = closing(rest, 0, '[', ']')?;
            let inner = rest[1..close].trim();
            if let Some(expression) = inner.strip_prefix("?(").and_then(|v| v.strip_suffix(')')) {
                steps.push(Step::Wildcard);
                let pred = if let Some((left, right)) = expression.split_once("==") {
                    Predicate::Equal(parse(left, depth + 1)?, literal(right)?, false)
                } else if let Some((left, right)) = expression.split_once("!=") {
                    Predicate::Equal(parse(left, depth + 1)?, literal(right)?, true)
                } else {
                    Predicate::Has(parse(expression, depth + 1)?)
                };
                steps.push(Step::Predicate(pred));
            } else if inner.contains(':') && !inner.starts_with(['\'', '"']) {
                let parts = inner.split(':').collect::<Vec<_>>();
                if !(2..=3).contains(&parts.len()) {
                    return Err(invalid("invalid slice"));
                }
                let num = |s: &str| {
                    if s.is_empty() {
                        Ok(None)
                    } else {
                        s.parse()
                            .map(Some)
                            .map_err(|_| invalid("invalid slice index"))
                    }
                };
                let stride = parts
                    .get(2)
                    .map(|v| num(v))
                    .transpose()?
                    .flatten()
                    .unwrap_or(1);
                if stride == 0 {
                    return Err(invalid("slice stride is zero"));
                }
                steps.push(Step::Slice(num(parts[0])?, num(parts[1])?, stride));
            } else {
                let parts = split_union(inner)?;
                let mut union = Vec::new();
                for part in parts {
                    union.push(if part == "*" {
                        Step::Wildcard
                    } else if part.starts_with(['\'', '"']) {
                        Step::Key(unquote(part)?)
                    } else {
                        Step::Index(part.parse().map_err(|_| invalid("invalid array index"))?)
                    });
                }
                if union.len() == 1 {
                    steps.push(union.pop().unwrap());
                } else {
                    steps.push(Step::Union(union));
                }
            }
            rest = &rest[close + 1..];
            if !rest.is_empty() && !rest.starts_with(['.', '[', ':']) {
                return Err(invalid("expected path separator"));
            }
        } else {
            let end = rest.find(['.', '[', ':']).unwrap_or(rest.len());
            if end == 0 {
                return Err(invalid("empty path key"));
            }
            let key = &rest[..end];
            if key
                .chars()
                .any(|c| c.is_whitespace() || "()>!=?&|]".contains(c))
            {
                return Err(invalid("unsupported path syntax"));
            }
            steps.push(if key == "*" {
                Step::Wildcard
            } else {
                Step::Key(key.to_owned())
            });
            rest = &rest[end..];
        }
    }
    Ok(steps)
}
fn closing(text: &str, start: usize, open: char, close: char) -> Result<usize, EngineError> {
    let mut level = 0_usize;
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in text.char_indices().skip_while(|(i, _)| *i < start) {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            continue;
        }
        if c == open {
            level += 1;
            if level > MAX_DEPTH {
                return Err(invalid("nesting depth exceeded"));
            }
        }
        if c == close {
            level = level
                .checked_sub(1)
                .ok_or_else(|| invalid("unbalanced selector"))?;
            if level == 0 {
                return Ok(i);
            }
        }
    }
    Err(invalid("unclosed selector"))
}
fn unquote(value: &str) -> Result<String, EngineError> {
    if value.starts_with('"') {
        return serde_json::from_str(value).map_err(|_| invalid("invalid quoted key"));
    }
    if let Some(value) = value.strip_prefix('\'') {
        return value
            .strip_suffix('\'')
            .map(|v| v.replace("\\'", "'").replace("\\\\", "\\"))
            .ok_or_else(|| invalid("unclosed quoted key"));
    }
    Ok(value.to_owned())
}
fn literal(value: &str) -> Result<JsonValue, EngineError> {
    let value = value.trim();
    if value.starts_with('\'') {
        return unquote(value).map(JsonValue::String);
    }
    serde_json::from_str(value).map_err(|_| invalid("invalid predicate literal"))
}
fn split_union(inner: &str) -> Result<Vec<&str>, EngineError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == ',' {
            parts.push(inner[start..i].trim());
            start = i + 1;
        }
    }
    if quote.is_some() {
        return Err(invalid("unclosed union key"));
    }
    parts.push(inner[start..].trim());
    if parts.iter().any(|v| v.is_empty()) {
        return Err(invalid("empty union item"));
    }
    Ok(parts)
}
