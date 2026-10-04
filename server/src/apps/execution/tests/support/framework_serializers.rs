//! Schema shapes used by the application's registry and event contracts.
use reinhardt::rest::openapi::{Schema, ToSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Schema)]
pub struct Metadata {
	pub payload: Value,
	pub translations: BTreeMap<String, String>,
}
