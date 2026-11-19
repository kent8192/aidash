use super::*;
use aidash_domain::federation::tool_lists::{HostPattern, ToolList, ToolPattern};

fn request() -> FederationToolRequest {
	FederationToolRequest {
		tools: ToolList::new([ToolPattern::new("file_*").unwrap()]).unwrap(),
		hosts: vec![HostPattern::new("*.example.com").unwrap()],
	}
}

#[test]
fn another_tenant_receiver_gets_one_detail_with_the_request() {
	let request = request();
	let details = authorization_details(Some(&request));
	assert_eq!(details.len(), 1);
	assert_eq!(details[0].request().unwrap(), request);
}

#[test]
fn a_receiver_without_registration_gets_no_detail() {
	assert!(authorization_details(None).is_empty());
}

fn asking(tools: &[&str]) -> FederationToolRequest {
	FederationToolRequest {
		tools: ToolList::new(tools.iter().map(|tool| ToolPattern::new(*tool).unwrap())).unwrap(),
		hosts: vec![],
	}
}

fn unavailable(result: Result<()>) -> bool {
	matches!(result, Err(Error::Invalid(message)) if message == aidash_domain::federation::tool_lists::UNAVAILABLE_TOOL)
}

#[test]
fn the_home_serves_a_tool_its_request_names() {
	let request = asking(&["workspace_search", "memory_*"]);
	require_requested(Some(&request), "workspace_search").unwrap();
	require_requested(Some(&request), "memory_mutate").unwrap();
}

#[test]
fn the_home_refuses_a_tool_its_request_does_not_name_whatever_the_receiver_sent() {
	let request = asking(&["workspace_message"]);
	for tool in [
		"workspace_search",
		"task_assign",
		"memory_mutate",
		"connection_request",
		"outbound_get",
		"file_share",
	] {
		assert!(
			unavailable(require_requested(Some(&request), tool)),
			"{tool}"
		);
	}
}

#[test]
fn a_run_only_answer_inside_the_request_is_served() {
	// The Home's "this Run only" answer joins the Request the Description carries.
	let run_only = ToolList::new([ToolPattern::new("outbound_get").unwrap()]).unwrap();
	let request = asking(&["workspace_message"])
		.with_run_tools(&run_only)
		.unwrap();
	require_requested(Some(&request), "outbound_get").unwrap();
}

#[test]
fn reach_never_and_unknown_tools_are_refused_even_under_a_wildcard() {
	let request = asking(&["*"]);
	assert!(unavailable(require_requested(Some(&request), "duty_list")));
	assert!(unavailable(require_requested(
		Some(&request),
		"no_such_tool"
	)));
}

#[test]
fn a_receiver_of_the_homes_own_tenant_has_no_lists() {
	require_requested(None, "workspace_search").unwrap();
	require_requested(None, "outbound_get").unwrap();
}
