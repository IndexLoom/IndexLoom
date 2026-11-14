//! `LazyLibrarian` uses display names, never its internal `Torznab_N`/`Newznab_N` names.
use super::{
    ApiError,
    named::{Entry, entries},
    transport::failure,
};
use serde_json::Value;

pub(super) fn parse(value: &Value) -> Result<Vec<Entry>, ApiError> {
    if value["Success"] != true {
        return Err(failure());
    }
    entries(&value["Data"], true)
}
