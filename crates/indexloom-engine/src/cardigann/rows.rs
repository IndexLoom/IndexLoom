//! Completed-release row filtering (separate from string filters).
use super::{
    EngineError, Mapping, YamlValue, key, mapping, scalar,
    transform::{arguments, error},
};
#[derive(Debug)]
pub(super) struct RowFilters {
    limits: Vec<Option<usize>>,
}
impl RowFilters {
    pub(super) fn parse(rows: &Mapping) -> Result<Self, EngineError> {
        let mut limits = Vec::new();
        if let Some(filters) = rows.get(key("filters")) {
            let filters = filters
                .as_sequence()
                .ok_or_else(|| error("rows", "filters must be a sequence"))?;
            if filters.len() > 128 {
                return Err(error("rows", "too many filters"));
            }
            for filter in filters {
                let filter =
                    mapping(filter).ok_or_else(|| error("rows", "filter must be a mapping"))?;
                let name = filter
                    .get(key("name"))
                    .and_then(scalar)
                    .ok_or_else(|| error("rows", "filter name is required"))?;
                let args = arguments(filter)?;
                if args.len() > 1 {
                    return Err(error(&name, "wrong argument count"));
                }
                match name.as_str() {
                    "andmatch" => {
                        let limit = args
                            .first()
                            .map(|v| {
                                v.parse::<i64>()
                                    .map_err(|_| error(&name, "character limit must be an integer"))
                            })
                            .transpose()?;
                        limits.push(
                            limit
                                .filter(|v| *v > 0)
                                .and_then(|v| usize::try_from(v).ok()),
                        );
                    }
                    "strdump" | "hexdump" => {}
                    _ => return Err(error(&name, "unknown row filter")),
                }
            }
        }
        Ok(Self { limits })
    }
    pub(super) fn allows(&self, title: &str, keywords: &str) -> bool {
        let title = title.to_lowercase();
        self.limits.iter().all(|limit| {
            let query: String = keywords.chars().take(limit.unwrap_or(usize::MAX)).collect();
            query
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|v| v.chars().count() > 1 && !["the", "and", "an"].contains(v))
                .all(|word| title.contains(word))
        })
    }
}
pub(super) fn rows(search: &Mapping) -> Result<&Mapping, EngineError> {
    let rows =
        search
            .get(key("rows"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search rows must be a mapping",
            ))?;
    if rows
        .get(key("after"))
        .is_some_and(|v| v.as_u64().is_none_or(|v| v > 100))
    {
        return Err(EngineError::CardigannIncompatible(
            "rows.after must be an integer between 0 and 100",
        ));
    }
    for option in ["multiple", "missingAttributeEqualsNoResults", "optional"] {
        if rows.get(key(option)).is_some_and(|v| v.as_bool().is_none()) {
            return Err(EngineError::InvalidSearchDefinition(
                "row option must be boolean",
            ));
        }
    }
    Ok(rows)
}
pub(super) fn field_specs(search: &Mapping) -> Result<&Mapping, EngineError> {
    search
        .get(key("fields"))
        .and_then(mapping)
        .ok_or(EngineError::InvalidSearchDefinition(
            "search fields must be a mapping",
        ))
}
pub(super) fn boolean(map: &Mapping, name: &str) -> bool {
    map.get(key(name))
        .and_then(YamlValue::as_bool)
        .unwrap_or(false)
}
