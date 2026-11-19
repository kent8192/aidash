//! What a Home asks a receiving Node for, carried by its Description (ADR 0139).
use crate::{Error, Result};
use aidash_domain::{
	federation::{
		execution::Description,
		tool_lists::{
			FederationToolRequest, RemoteToolLists, ToolAuthorizationDetail, ToolList, ToolPattern,
			ToolVerdict, UNAVAILABLE_TOOL,
		},
	},
	tool::ToolReach,
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

/// Whether the Home may serve `tool`, a builtin it executes itself for a Run that runs on
/// another Node (a Home-forwarded tool or a delivery), under `request`: the Home's current
/// Request for that Node with this grant's Run-only answers, as the Description it rebuilt for
/// the call carries it (ADR 0139, decision 1). The Home never relies on the receiving Node's
/// gate for its own key, so it decides its half of the lists here with the same decision the
/// receiver uses, the receiver's half standing in as granted. `None` is a receiver of the Home's
/// own Tenant: no lists, the Tenant's policy decides. A tool the Request does not name, Reach
/// never, or a tool no Reach table knows answers the words of a tool that does not exist
/// (decision 14).
pub fn require_requested(request: Option<&FederationToolRequest>, tool: &str) -> Result<()> {
	let reach = aidash_domain::tool::builtin_reach(tool).ok_or_else(unavailable)?;
	require_requested_key(request, reach, tool)
}

/// [`require_requested`] for a list `key` that is not a tool's own name, such as the key a
/// message's files are listed under, decided with the `reach` of the tool it belongs to.
pub fn require_requested_key(
	request: Option<&FederationToolRequest>,
	reach: ToolReach,
	key: &str,
) -> Result<()> {
	let Some(request) = request else {
		return Ok(());
	};
	let granted = ToolList::new([ToolPattern::new("*")?])?;
	let none = ToolList::default();
	let lists = RemoteToolLists {
		request: &request.tools,
		allowance: &granted,
		block: &none,
		// Run-only answers of the Home already travel inside its Request.
		run_request: &none,
		run_allowance: &none,
	};
	match lists.decide(reach, key) {
		ToolVerdict::Usable => Ok(()),
		ToolVerdict::Ask(_) | ToolVerdict::Unavailable => Err(unavailable()),
	}
}

fn unavailable() -> Error {
	Error::Invalid(UNAVAILABLE_TOOL.into())
}

#[cfg(test)]
mod tests;
