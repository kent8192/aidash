use super::*;

fn list(patterns: &[&str]) -> ToolList {
	ToolList::new(patterns.iter().map(|p| ToolPattern::new(*p).unwrap())).unwrap()
}

#[test]
fn patterns_match_names_and_globs() {
	let file = ToolPattern::new("file_*").unwrap();
	assert!(file.matches("file_read"));
	assert!(!file.matches("shell"));
	let github = ToolPattern::new("registry:github/*").unwrap();
	assert!(github.matches("registry:github/create_issue"));
	assert!(!github.matches("registry:gitlab/create_issue"));
	assert!(ToolPattern::new("*").unwrap().matches("anything/at:all"));
	assert!(ToolPattern::new("a*b*c").unwrap().matches("axxbyyc"));
	assert!(!ToolPattern::new("a*b*c").unwrap().matches("axxbyy"));
	assert!(ToolPattern::new("shell").unwrap().matches("shell"));
	assert!(!ToolPattern::new("shell").unwrap().matches("shell_poll"));
}

#[test]
fn patterns_reject_bad_input() {
	for bad in [
		"",
		"File_read",
		"a b",
		"x;y",
		&"a".repeat(MAX_PATTERN_BYTES + 1),
	] {
		assert!(ToolPattern::new(bad).is_err(), "{bad:?}");
	}
	assert!(
		HostPattern::new("*.github.com")
			.unwrap()
			.matches("API.github.com")
	);
	assert!(HostPattern::new("bad/host").is_err());
}

#[test]
fn lists_sort_dedupe_and_cap() {
	let l = list(&["b", "a", "b"]);
	assert_eq!(l.patterns().len(), 2);
	assert_eq!(l.patterns()[0].as_str(), "a");
	let too_many = (0..=MAX_PATTERNS).map(|i| ToolPattern::new(format!("t{i}")).unwrap());
	assert!(ToolList::new(too_many).is_err());
	assert!(ToolList::defaults().matches("skill_read"));
	assert!(!ToolList::defaults().matches("file_read"));
}

#[test]
fn both_keys_decide_and_the_block_wins() {
	let request = list(&["file_*", "shell"]);
	let allowance = list(&["file_read", "file_search", "shell"]);
	let block = list(&["shell"]);
	let empty = ToolList::default();
	let lists = RemoteToolLists {
		request: &request,
		allowance: &allowance,
		block: &block,
		run_request: &empty,
		run_allowance: &empty,
	};
	assert_eq!(
		lists.decide(ToolReach::ReceiverMaterial, "file_read"),
		ToolVerdict::Usable
	);
	assert_eq!(
		lists.decide(ToolReach::Execution, "shell"),
		ToolVerdict::Unavailable
	);
	assert_eq!(
		lists.decide(ToolReach::HomeWorkspace, "file_share"),
		ToolVerdict::Ask(Lacking {
			home: false,
			receiver: true
		})
	);
	assert_eq!(
		lists.decide(ToolReach::Execution, "code_interpreter"),
		ToolVerdict::Ask(Lacking {
			home: true,
			receiver: true
		})
	);
	assert_eq!(
		lists.decide(ToolReach::Run, "journal_read"),
		ToolVerdict::Usable
	);
	assert_eq!(
		lists.decide(ToolReach::Never, "file_read"),
		ToolVerdict::Unavailable
	);
}

#[test]
fn run_only_answers_fill_the_missing_side() {
	let request = list(&["file_share"]);
	let empty = ToolList::default();
	let run_allowance = list(&["file_share"]);
	let lists = RemoteToolLists {
		request: &request,
		allowance: &empty,
		block: &empty,
		run_request: &empty,
		run_allowance: &run_allowance,
	};
	assert_eq!(
		lists.decide(ToolReach::HomeWorkspace, "file_share"),
		ToolVerdict::Usable
	);
}

#[test]
fn discovery_shows_the_allowance_minus_the_block() {
	let allowance = list(&["file_*"]);
	let block = list(&["file_share"]);
	assert!(RemoteToolLists::discoverable(
		&allowance,
		&block,
		"file_read"
	));
	assert!(!RemoteToolLists::discoverable(
		&allowance,
		&block,
		"file_share"
	));
	assert!(!RemoteToolLists::discoverable(&allowance, &block, "shell"));
}

#[test]
fn the_request_round_trips_as_an_authorization_detail() {
	let request = FederationToolRequest {
		tools: list(&["file_read"]),
		hosts: vec![HostPattern::new("*.example.com").unwrap()],
	};
	let detail = ToolAuthorizationDetail::new(&request);
	let json = serde_json::to_value(&detail).unwrap();
	assert_eq!(json["type"], AUTHORIZATION_DETAIL_TYPE);
	assert_eq!(json["actions"][0], "file_read");
	assert_eq!(json["locations"][0], "*.example.com");
	let back: ToolAuthorizationDetail = serde_json::from_value(json).unwrap();
	assert_eq!(back.request().unwrap(), request);
	assert!(request.allows_host("api.example.com"));
	assert!(!request.allows_host("example.org"));
	let other = ToolAuthorizationDetail {
		kind: "other".into(),
		..detail
	};
	assert!(other.request().is_err());
}
