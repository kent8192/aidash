//! Bounded browsing and administration parameters.
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
fn page_size() -> usize {
	50
}

#[derive(Deserialize, Serialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Browse {
	#[serde(default)]
	#[validate(length(max = 256))]
	pub(crate) q: String,
	#[serde(default)]
	#[validate(range(max = 10000))]
	pub(crate) offset: usize,
	#[serde(default = "page_size")]
	#[validate(range(min = 1, max = 100))]
	pub(crate) limit: usize,
}

#[derive(Deserialize, Serialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceQuery {
	#[serde(default)]
	#[validate(range(max = 10000))]
	pub(crate) offset: usize,
	#[serde(default = "page_size")]
	#[validate(range(min = 1, max = 100))]
	pub(crate) limit: usize,
}

#[derive(Deserialize, Serialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionQuery {
	pub(crate) revision: Option<i64>,
}

#[derive(Deserialize, Serialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdministrationQuery {
	pub(crate) tenant: String,
	#[serde(default)]
	#[validate(range(max = 10000))]
	pub(crate) offset: usize,
	#[serde(default = "page_size")]
	#[validate(range(min = 1, max = 100))]
	pub(crate) limit: usize,
}
