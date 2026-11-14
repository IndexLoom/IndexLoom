//! Mylar3's command API; numeric IDs are scoped to provider kind.
use super::{
    ApiError,
    named::{Entry, entries},
    transport::failure,
};
use serde_json::Value;

pub(super) fn parse(value: &Value) -> Result<Vec<Entry>, ApiError> {
    if value["success"] != true {
        return Err(failure());
    }
    entries(&value["data"], false)
}
