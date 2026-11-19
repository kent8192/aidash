//! A Workspace search the Home runs for a Run that a Remote Grant started (ADR 0139).
//!
//! The Run has no Run row, Working Area or Reference documents on the Home, so the query is
//! text only. The delegation chain must be allowed both to search and to have the results
//! disclosed, since the answer leaves the Home.
use crate::{Error, Result, authorization::access::Access, store::Store};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) async fn workspace_search(
	access: &mut Access,
	store: &Store,
	workspace: Uuid,
	input: &Value,
) -> Result<Value> {
	let mut lease = crate::semantic::service::Lease::Inherited(access);
	lease.durable();
	lease.lists();
	lease.workspace(workspace, "semantic.search").await?;
	lease.workspace(workspace, "semantic.disclose").await?;
	let index =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), workspace, false)
			.await?;
	let spec: Option<aidash_domain::semantic::indexing::IndexingSpec> = index
		.map(|index| serde_json::from_value(index.spec))
		.transpose()?;
	let limit = input["limit"].as_u64().map(|value| value as usize);
	let max_tokens = input["max_tokens"].as_u64().map(|value| value as usize);
	if limit.is_some_and(|value| value == 0 || value > 20)
		|| max_tokens.is_some_and(|value| value == 0 || value > 32768)
	{
		return Err(Error::Invalid("invalid semantic search limits".into()));
	}
	let enabled = spec.as_ref().is_some_and(|spec| spec.enabled);
	let mut result = if enabled {
		let spec = spec.as_ref().ok_or(Error::Forbidden)?;
		let search = crate::apps::knowledge::serializers::contracts::Search {
			query: input["query"].as_str().unwrap_or_default().to_owned(),
			image: None,
			attachment: None,
			agent: None,
			metadata: json!({}),
			limit: limit.map_or(spec.max_results.min(5), |value| value.min(spec.max_results)),
			max_tokens: max_tokens.map_or(spec.max_result_tokens, |value| {
				value.min(spec.max_result_tokens)
			}),
		};
		serde_json::to_value(
			crate::semantic::service::search_in(store, &mut lease, workspace, search, None, None)
				.await?,
		)?
	} else {
		let mut result = json!({"workspace_id":workspace, "matches":[], "estimated_tokens":0,
			"truncated":false, "workspace":{"status":"unavailable", "cause":if spec.is_some() {"disabled"} else {"missing"}}});
		result["estimated_tokens"] =
			json!(aidash_domain::context::estimated_tokens(&result.to_string()) + 16);
		result
	};
	aidash_domain::unconfirmed::for_tool(&mut result);
	Ok(result)
}
