//! Independently implemented string filter interpreter. No source values are logged.
use super::{
    EngineError, JsonValue, TemplateValue, YamlValue, key, mapping, render_template, scalar,
};
use super::{filters::cardigann_regex, jsonpath, transport::WireEncoding};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_VALUE: usize = 16 * 1024 * 1024;
pub(super) fn error(name: &str, reason: &'static str) -> EngineError {
    EngineError::CardigannFilter {
        name: name.to_owned(),
        reason,
    }
}
pub(super) fn arguments(filter: &serde_yaml::Mapping) -> Result<Vec<String>, EngineError> {
    let args = match filter.get(key("args")) {
        None | Some(YamlValue::Null) => Vec::new(),
        Some(YamlValue::Array(values)) => values
            .iter()
            .map(|v| scalar(v).ok_or_else(|| error("arguments", "arguments must be scalars")))
            .collect::<Result<_, _>>()?,
        Some(value) => {
            vec![scalar(value).ok_or_else(|| error("arguments", "arguments must be scalars"))?]
        }
    };
    Ok(args)
}
#[allow(
    clippy::too_many_lines,
    reason = "Keep the complete bounded filter dispatch table together."
)]
pub(super) fn apply(
    mut value: String,
    filters: &YamlValue,
    context: &TemplateValue,
    encoding: &str,
) -> Result<String, EngineError> {
    let filters = filters
        .as_array()
        .ok_or_else(|| error("pipeline", "filters must be a sequence"))?;
    if filters.len() > 128 {
        return Err(error("pipeline", "more than 128 filters"));
    }
    let wire = WireEncoding::new(encoding)?;
    for filter in filters {
        let filter =
            mapping(filter).ok_or_else(|| error("pipeline", "filter must be a mapping"))?;
        let name = filter
            .get(key("name"))
            .and_then(scalar)
            .ok_or_else(|| error("pipeline", "filter name is required"))?;
        let args = arguments(filter)?
            .iter()
            .map(|v| render_template(v, context))
            .collect::<Result<Vec<_>, _>>()?;
        if args.iter().any(|v| v.len() > MAX_VALUE) {
            return Err(error(&name, "argument exceeds size budget"));
        }
        let bounds = match name.as_str() {
            "replace" | "re_replace" | "split" | "jsonjoinarray" => (2, 2),
            "regexp" | "querystring" | "dateparse" | "timeparse" | "diacritics" | "validate"
            | "append" | "prepend" => (1, 1),
            "trim" | "strdump" | "fuzzytime" => (0, 1),
            "tolower" | "toupper" | "urlencode" | "urldecode" | "htmlencode" | "htmldecode"
            | "base64encode" | "base64decode" | "timeago" | "reltime" | "validfilename"
            | "hexdump" => (0, 0),
            _ => {
                return Err(error(
                    &name,
                    "unknown filter or filter is invalid in a string context",
                ));
            }
        };
        if !(bounds.0..=bounds.1).contains(&args.len()) {
            return Err(error(&name, "wrong argument count"));
        }
        if value.len() > MAX_VALUE {
            return Err(error(&name, "input exceeds 16 MiB limit"));
        }
        match name.as_str() {
            "replace" => {
                if args[0].is_empty() {
                    return Err(error(&name, "replacement pattern is empty"));
                }
                let occurrences = value.matches(&args[0]).count();
                if value
                    .len()
                    .saturating_add(occurrences.saturating_mul(args[1].len()))
                    > MAX_VALUE
                {
                    return Err(error(&name, "replacement exceeds size budget"));
                }
                value = value.replace(&args[0], &args[1]);
            }
            "re_replace" => value = regex_replace(&value, &args[0], &args[1])?,
            "regexp" => {
                let regex = cardigann_regex(&args[0])?;
                value = regex
                    .captures(&value)
                    .map_err(|_| error(&name, "regex execution budget exceeded"))?
                    .and_then(|v| v.get(1))
                    .map_or_else(String::new, |v| v.as_str().to_owned());
            }
            "split" => {
                let separator = args[0]
                    .chars()
                    .next()
                    .ok_or_else(|| error(&name, "separator is empty"))?;
                let pos = args[1]
                    .parse::<isize>()
                    .map_err(|_| error(&name, "split position must be an integer"))?;
                let parts: Vec<_> = value.split(separator).collect();
                let count = isize::try_from(parts.len())
                    .map_err(|_| error(&name, "too many split parts"))?;
                let pos = if pos < 0 {
                    count.checked_add(pos)
                } else {
                    Some(pos)
                };
                value = pos
                    .and_then(|p| usize::try_from(p).ok())
                    .and_then(|p| parts.get(p))
                    .ok_or_else(|| error(&name, "split position is out of range"))?
                    .to_string();
            }
            "trim" => {
                value = args.first().map_or_else(
                    || value.trim().to_owned(),
                    |a| value.trim_matches(|c| a.contains(c)).to_owned(),
                );
            }
            "append" => value.push_str(&args[0]),
            "prepend" => value = format!("{}{value}", args[0]),
            "tolower" => value = value.to_lowercase(),
            "toupper" => value = value.to_uppercase(),
            "urlencode" => value = wire.escape(&value)?,
            "urldecode" => value = url_decode(&value, &wire)?,
            "querystring" => {
                let query = value
                    .split_once('?')
                    .map_or(value.as_str(), |(_, q)| q)
                    .split('#')
                    .next()
                    .unwrap_or_default();
                let mut found = String::new();
                for pair in query.split('&') {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    if url_decode(k, &wire)? == args[0] {
                        found = url_decode(v, &wire)?;
                        break;
                    }
                }
                value = found;
            }
            "base64encode" => value = URL_SAFE_NO_PAD.encode(value.as_bytes()),
            "base64decode" => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(&value)
                    .or_else(|_| URL_SAFE.decode(&value))
                    .or_else(|_| STANDARD.decode(&value))
                    .or_else(|_| STANDARD_NO_PAD.decode(&value))
                    .map_err(|_| error(&name, "invalid base64"))?;
                value = String::from_utf8(bytes)
                    .map_err(|_| error(&name, "base64 payload is not UTF-8"))?;
            }
            "htmlencode" => value = html_encode(&value),
            "htmldecode" => value = html_decode(&value),
            "diacritics" => {
                if args[0] != "replace" {
                    return Err(error(&name, "only replace is supported"));
                }
                value = value
                    .nfd()
                    .filter(|c| !is_combining_mark(*c))
                    .nfc()
                    .collect();
            }
            "validfilename" => {
                value = value
                    .chars()
                    .map(|c| {
                        if c.is_control() || "<>:\"/\\|?*".contains(c) {
                            '_'
                        } else {
                            c
                        }
                    })
                    .collect();
            }
            "jsonjoinarray" => {
                let json: JsonValue =
                    serde_json::from_str(&value).map_err(|_| error(&name, "invalid JSON"))?;
                let selected = jsonpath::select(&json, &args[0])?;
                let values: Vec<_> = selected
                    .iter()
                    .flat_map(|v| {
                        v.as_array()
                            .map_or_else(|| vec![*v], |a| a.iter().collect())
                    })
                    .map(super::extract::json_string)
                    .collect();
                value = values.join(&args[1]);
            }
            "validate" => {
                let tokens = |v: &str| {
                    v.to_lowercase()
                        .split(|c: char| ", /)(.;[]\"|:".contains(c))
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                let input = tokens(&value);
                let mut kept = Vec::new();
                for token in tokens(&args[0]) {
                    if input.contains(&token) && !kept.contains(&token) {
                        kept.push(token);
                    }
                }
                value = kept.join(", ");
            }
            "dateparse" | "timeparse" | "timeago" | "reltime" | "fuzzytime" => {
                value =
                    super::dates::filter(&name, &value, &args, time::OffsetDateTime::now_utc())?;
            }
            // Upstream diagnostic filters preserve data. Deliberately do not log potentially secret input.
            "strdump" | "hexdump" => {}
            _ => unreachable!(),
        }
        if value.len() > MAX_VALUE {
            return Err(error(&name, "output exceeds 16 MiB limit"));
        }
    }
    Ok(value)
}

pub(super) fn regex_replace(
    input: &str,
    pattern: &str,
    replacement: &str,
) -> Result<String, EngineError> {
    if input.len() > MAX_VALUE || replacement.len() > MAX_VALUE {
        return Err(error(
            "re_replace",
            "input or replacement exceeds size budget",
        ));
    }
    let regex = cardigann_regex(pattern)?;
    let mut output = String::new();
    let mut end = 0;
    for (count, captures) in regex.captures_iter(input).enumerate() {
        if count > 100_000 {
            return Err(error("re_replace", "too many matches"));
        }
        let captures =
            captures.map_err(|_| error("re_replace", "regex execution budget exceeded"))?;
        let matched = captures
            .get(0)
            .ok_or_else(|| error("re_replace", "missing match"))?;
        append_bounded(&mut output, &input[end..matched.start()])?;
        let mut rest = replacement;
        while let Some(pos) = rest.find('$') {
            append_bounded(&mut output, &rest[..pos])?;
            rest = &rest[pos + 1..];
            if let Some(tail) = rest.strip_prefix('$') {
                append_bounded(&mut output, "$")?;
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix('&') {
                append_bounded(&mut output, matched.as_str())?;
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix('`') {
                append_bounded(&mut output, &input[..matched.start()])?;
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix('\'') {
                append_bounded(&mut output, &input[matched.end()..])?;
                rest = tail;
            } else {
                let (name, consumed) = if let Some(tail) = rest.strip_prefix('{') {
                    if let Some(close) = tail.find('}') {
                        (&tail[..close], close + 2)
                    } else {
                        ("", 0)
                    }
                } else {
                    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                    (&rest[..digits], digits)
                };
                let capture = if let Ok(index) = name.parse::<usize>() {
                    captures.get(index)
                } else {
                    captures.name(name)
                };
                if consumed > 0 && !name.is_empty() {
                    if let Some(capture) = capture {
                        append_bounded(&mut output, capture.as_str())?;
                    } else {
                        append_bounded(&mut output, "$")?;
                        append_bounded(&mut output, &rest[..consumed])?;
                    }
                    rest = &rest[consumed..];
                } else {
                    append_bounded(&mut output, "$")?;
                }
            }
        }
        append_bounded(&mut output, rest)?;
        end = matched.end();
    }
    if output.len().saturating_add(input.len() - end) > MAX_VALUE {
        return Err(error("re_replace", "replacement exceeds size budget"));
    }
    output.push_str(&input[end..]);
    Ok(output)
}
fn append_bounded(output: &mut String, value: &str) -> Result<(), EngineError> {
    if output.len().saturating_add(value.len()) > MAX_VALUE {
        return Err(error("re_replace", "replacement exceeds size budget"));
    }
    output.push_str(value);
    Ok(())
}
fn url_decode(value: &str, wire: &WireEncoding) -> Result<String, EngineError> {
    let mut output = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        if rest.starts_with('%') {
            let mut encoded = Vec::new();
            while rest.starts_with('%') {
                let pair = rest
                    .get(1..3)
                    .ok_or_else(|| error("urldecode", "incomplete percent escape"))?;
                encoded.push(
                    u8::from_str_radix(pair, 16)
                        .map_err(|_| error("urldecode", "invalid percent escape"))?,
                );
                rest = &rest[3..];
            }
            output.push_str(&wire.decode(&encoded)?);
        } else {
            let c = rest.chars().next().expect("nonempty source");
            output.push(if c == '+' { ' ' } else { c });
            rest = &rest[c.len_utf8()..];
        }
    }
    Ok(output)
}
fn html_encode(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            c => c.to_string(),
        })
        .collect()
}
fn html_decode(value: &str) -> String {
    // HTML5 entity table also decodes entities in text containing literal markup, without parsing it as HTML.
    let mut output = String::new();
    let mut rest = value;
    while let Some(pos) = rest.find('&') {
        output.push_str(&rest[..pos]);
        rest = &rest[pos + 1..];
        let mut decoded = None;
        if let Some(end) = rest.find(';').filter(|e| *e <= 32) {
            let entity = &rest[..end];
            if let Some(n) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|v| u32::from_str_radix(v, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|v| v.parse::<u32>().ok()))
                .and_then(char::from_u32)
            {
                decoded = Some((n.to_string(), end + 1));
            }
        }
        if decoded.is_none() {
            for end in (1..=rest.len().min(33))
                .rev()
                .filter(|e| rest.is_char_boundary(*e))
            {
                if let Some(&(a, b)) = markup5ever::data::NAMED_ENTITIES.get(&rest[..end]) {
                    let mut s = char::from_u32(a).unwrap_or('\u{fffd}').to_string();
                    if b != 0 {
                        s.push(char::from_u32(b).unwrap_or('\u{fffd}'));
                    }
                    decoded = Some((s, end));
                    break;
                }
            }
        }
        if let Some((text, len)) = decoded {
            output.push_str(&text);
            rest = &rest[len..];
        } else {
            output.push('&');
        }
    }
    output.push_str(rest);
    output
}
