//! CSS selection with text predicates at the correct compound/ancestor position.
use super::{ElementRef, EngineError, Selector};
#[derive(Debug)]
pub(super) struct Css {
    groups: Vec<Vec<Part>>,
}
#[derive(Debug)]
struct Part {
    native: Selector,
    relation: char,
    tests: Vec<Test>,
}
#[derive(Debug)]
enum Test {
    Contains(String),
    Has(Css),
    Not(Css),
    Is(Css),
}
fn invalid() -> EngineError {
    EngineError::InvalidSelector("invalid or excessively complex Cardigann CSS selector".into())
}
impl Css {
    pub(super) fn parse(source: &str) -> Result<Self, EngineError> {
        let css = Self::parse_depth(source, 0)?;
        if css.groups.iter().any(|g| g[0].relation != ' ') {
            return Err(invalid());
        }
        Ok(css)
    }
    fn parse_depth(source: &str, depth: usize) -> Result<Self, EngineError> {
        if depth > 16 || source.len() > 8192 {
            return Err(invalid());
        }
        let mut groups = Vec::new();
        for group in split_top(source, ',')? {
            let mut parts = Vec::new();
            let mut start = 0;
            let mut relation = ' ';
            let mut i = 0;
            let bytes = group.as_bytes();
            let mut quote = 0;
            let mut level = 0;
            let mut escaped = false;
            while i <= bytes.len() {
                let c = bytes.get(i).copied().unwrap_or(b' ');
                if escaped {
                    escaped = false;
                    i += 1;
                    continue;
                }
                if c == b'\\' {
                    escaped = true;
                    i += 1;
                    continue;
                }
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                    i += 1;
                    continue;
                }
                if c == b'\'' || c == b'"' {
                    quote = c;
                    i += 1;
                    continue;
                }
                if c == b'(' || c == b'[' {
                    level += 1;
                }
                if c == b')' || c == b']' {
                    level -= 1;
                }
                if level == 0 && (c.is_ascii_whitespace() || b">+~".contains(&c)) {
                    if !group[start..i].trim().is_empty() {
                        parts.push(compile_part(group[start..i].trim(), relation, depth)?);
                        relation = ' ';
                    }
                    if b">+~".contains(&c) {
                        if relation != ' ' {
                            return Err(invalid());
                        }
                        relation = char::from(c);
                    }
                    start = i + 1;
                }
                i += 1;
            }
            if parts.is_empty() || parts.len() > 64 || relation != ' ' {
                return Err(invalid());
            }
            groups.push(parts);
        }
        if groups.len() > 64 {
            return Err(invalid());
        }
        Ok(Self { groups })
    }
    fn matches_budget(
        &self,
        element: ElementRef<'_>,
        budget: &mut usize,
        anchor: Option<ElementRef<'_>>,
    ) -> Result<bool, EngineError> {
        for parts in &self.groups {
            if match_parts(parts, parts.len() - 1, element, budget, anchor)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn select<'a>(
        &self,
        root: ElementRef<'a>,
        include_root: bool,
    ) -> Result<Vec<ElementRef<'a>>, EngineError> {
        let mut budget = 200_000;
        let mut result = Vec::new();
        for element in root.descendants().filter_map(ElementRef::wrap) {
            if !include_root && element.id() == root.id() {
                continue;
            }
            if self.matches_budget(element, &mut budget, Some(root))? {
                result.push(element);
            }
        }
        Ok(result)
    }
}
fn spend(budget: &mut usize) -> Result<(), EngineError> {
    *budget = budget.checked_sub(1).ok_or_else(invalid)?;
    Ok(())
}
fn match_parts(
    parts: &[Part],
    index: usize,
    element: ElementRef<'_>,
    budget: &mut usize,
    anchor: Option<ElementRef<'_>>,
) -> Result<bool, EngineError> {
    spend(budget)?;
    let part = &parts[index];
    if !part.native.matches_with_scope(&element, anchor) {
        return Ok(false);
    }
    for test in &part.tests {
        let pass = match test {
            Test::Contains(needle) => {
                let mut text = String::new();
                for node in element.descendants() {
                    spend(budget)?;
                    if let Some(value) = node.value().as_text() {
                        *budget = budget.checked_sub(value.len() / 256).ok_or_else(invalid)?;
                        text.push_str(value);
                    }
                }
                text.contains(needle)
            }
            Test::Is(css) => css.matches_budget(element, budget, anchor)?,
            Test::Not(css) => !css.matches_budget(element, budget, anchor)?,
            Test::Has(css) => {
                let mut found = false;
                let mut candidates = Vec::new();
                for child in element
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .filter(|e| e.id() != element.id())
                {
                    spend(budget)?;
                    candidates.push(child);
                }
                if css
                    .groups
                    .iter()
                    .any(|g| matches!(g[0].relation, '+' | '~'))
                {
                    for sibling in element.next_siblings().filter_map(ElementRef::wrap) {
                        for child in sibling.descendants().filter_map(ElementRef::wrap) {
                            spend(budget)?;
                            candidates.push(child);
                        }
                    }
                }
                for child in candidates {
                    if css.matches_budget(child, budget, Some(element))? {
                        found = true;
                        break;
                    }
                }
                found
            }
        };
        if !pass {
            return Ok(false);
        }
    }
    if index == 0 {
        return Ok(anchor.is_none_or(|root| match part.relation {
            '>' => element.parent().is_some_and(|p| p.id() == root.id()),
            '+' => element
                .prev_siblings()
                .find_map(ElementRef::wrap)
                .is_some_and(|p| p.id() == root.id()),
            '~' => element.prev_siblings().any(|p| p.id() == root.id()),
            _ => true,
        }));
    }
    match part.relation {
        '>' => element
            .parent()
            .and_then(ElementRef::wrap)
            .map_or(Ok(false), |p| {
                match_parts(parts, index - 1, p, budget, anchor)
            }),
        '+' => element
            .prev_siblings()
            .find_map(ElementRef::wrap)
            .map_or(Ok(false), |p| {
                match_parts(parts, index - 1, p, budget, anchor)
            }),
        '~' => {
            for sibling in element.prev_siblings().filter_map(ElementRef::wrap) {
                if match_parts(parts, index - 1, sibling, budget, anchor)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => {
            for parent in element.ancestors().filter_map(ElementRef::wrap) {
                if match_parts(parts, index - 1, parent, budget, anchor)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}
fn compile_part(source: &str, relation: char, depth: usize) -> Result<Part, EngineError> {
    let mut native = String::new();
    let mut tests = Vec::new();
    let mut rest = source;
    let mut quote = None;
    let mut bracket = 0;
    while !rest.is_empty() {
        let c = rest.chars().next().ok_or_else(invalid)?;
        if c == '\\' {
            native.push(c);
            rest = &rest[1..];
            let c = rest.chars().next().ok_or_else(invalid)?;
            native.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == '[' {
            bracket += 1;
        } else if c == ']' {
            bracket -= 1;
        } else if c == ':' && bracket == 0 {
            let name = ["contains", "has", "not", "is"]
                .iter()
                .find(|name| rest.starts_with(&format!(":{name}(")));
            if let Some(name) = name {
                let open = name.len() + 1;
                let close = close_paren(rest, open)?;
                let content = &rest[open + 1..close];
                // Keep ordinary CSS :has/:not/:is in the native engine, which supports relative selectors.
                if *name != "contains" && !content.contains(":contains(") {
                    native.push_str(&rest[..=close]);
                } else {
                    tests.push(match *name {
                        "contains" => {
                            Test::Contains(content.trim().trim_matches(['\'', '"']).to_owned())
                        }
                        "has" => Test::Has(Css::parse_depth(content, depth + 1)?),
                        "not" => Test::Not(Css::parse_depth(content, depth + 1)?),
                        _ => Test::Is(Css::parse_depth(content, depth + 1)?),
                    });
                }
                rest = &rest[close + 1..];
                continue;
            }
        }
        native.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if native.is_empty() {
        native.push('*');
    }
    Ok(Part {
        native: Selector::parse(&native).map_err(|_| invalid())?,
        relation,
        tests,
    })
}
fn close_paren(source: &str, start: usize) -> Result<usize, EngineError> {
    let mut level = 0;
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in source.char_indices().skip_while(|(i, _)| *i < start) {
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
        } else if c == '(' {
            level += 1;
            if level > 16 {
                return Err(invalid());
            }
        } else if c == ')' {
            level -= 1;
            if level == 0 {
                return Ok(i);
            }
        }
    }
    Err(invalid())
}
fn split_top(source: &str, separator: char) -> Result<Vec<&str>, EngineError> {
    let mut level = 0_i32;
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    let mut groups = Vec::new();
    for (i, c) in source.char_indices() {
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
        } else if c == '(' || c == '[' {
            level += 1;
            if level > 32 {
                return Err(invalid());
            }
        } else if c == ')' || c == ']' {
            level -= 1;
            if level < 0 {
                return Err(invalid());
            }
        } else if c == separator && level == 0 {
            groups.push(source[start..i].trim());
            start = i + c.len_utf8();
        }
    }
    if level != 0 || quote.is_some() {
        return Err(invalid());
    }
    groups.push(source[start..].trim());
    Ok(groups)
}

pub(super) fn text(element: ElementRef<'_>, remove: Option<&str>) -> Result<String, EngineError> {
    let remove = remove.map(Css::parse).transpose()?;
    let removed = remove
        .map(|s| s.select(element, false))
        .transpose()?
        .unwrap_or_default();
    let mut text = String::new();
    for node in element.descendants() {
        if let Some(value) = node.value().as_text()
            && !node
                .ancestors()
                .any(|p| removed.iter().any(|r| r.id() == p.id()))
        {
            text.push_str(value);
        }
    }
    Ok(text.trim().to_owned())
}

/// Clone only a field's selected subtree, so `remove` affects cases and text
/// without mutating later field extractions or reparsing XML through HTML.
pub(super) fn without(element: ElementRef<'_>, remove: &str) -> Result<super::Html, EngineError> {
    let mut doc = super::Html::new_document();
    let root = doc
        .tree
        .root_mut()
        .append(scraper::Node::Element(element.value().clone()))
        .id();
    let mut pending = vec![(element.id(), root)];
    let mut budget = 200_000;
    while let Some((original, cloned)) = pending.pop() {
        let original = element.tree().get(original).ok_or_else(invalid)?;
        for child in original.children() {
            spend(&mut budget)?;
            let id = doc
                .tree
                .get_mut(cloned)
                .ok_or_else(invalid)?
                .append(child.value().clone())
                .id();
            pending.push((child.id(), id));
        }
    }
    let ids = Css::parse(remove)?
        .select(doc.root_element(), false)?
        .iter()
        .map(|n| n.id())
        .collect::<Vec<_>>();
    for id in ids {
        doc.tree.get_mut(id).ok_or_else(invalid)?.detach();
    }
    Ok(doc)
}
