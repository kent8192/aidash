//! What a Home asks a receiving Node for, carried by its Description (ADR 0139).
use crate::{Error, Result};
use aidash_domain::federation::{
	execution::Description,
	tool_lists::{FederationToolRequest, ToolAuthorizationDetail},
};

/// The Description entries for a Home's registered request: one entry, or none when the
/// Home registers no such Node (a receiver of its own Tenant), so the lists do not apply.
pub fn authorization_details(
	request: Option<&FederationToolRequest>,
) -> Vec<ToolAuthorizationDetail> {
	request
		.map(|request| vec![ToolAuthorizationDetail::new(request)])
		.unwrap_or_default()
}

/// The request a receiver reads from a Home's Description. `None` when the Description
/// carries none, which means the Home and the receiver share a Tenant and the lists do not
/// apply. A malformed or repeated entry is refused rather than read as "no lists".
pub fn requested_tools(description: &Description) -> Result<Option<FederationToolRequest>> {
	match description.authorization_details.as_slice() {
		[] => Ok(None),
		[detail] => detail
			.request()
			.map(Some)
			.map_err(|error| Error::Invalid(error.to_string())),
		_ => Err(Error::Invalid(
			"a Description carries at most one tool authorization detail".into(),
		)),
	}
}

#[cfg(test)]
mod tests;
