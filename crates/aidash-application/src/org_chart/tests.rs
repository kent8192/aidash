use super::*;
use chrono::{TimeZone, Utc};
use rstest::rstest;

fn agent(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}

fn spend(nanocredits: i64) -> Spend {
	Spend {
		nanocredits,
		unpriced_calls: 0,
	}
}

struct Fixture {
	rows: Rows,
	flow_root: Uuid,
	creating_task: Uuid,
}

/// Two Nodes: `research`, which alice's chain created, with a Duty of alice and
/// one of bob and a Subnode of each; and `sales`, which a person created.
fn fixture() -> Fixture {
	let flow_root = Uuid::new_v4();
	let creating_task = Uuid::new_v4();
	let duty = |node: &str, requester: &str, spent: Option<Uuid>| DutyRow {
		id: spent.unwrap_or_else(Uuid::new_v4),
		node: node.into(),
		title: format!("{requester}'s report"),
		requester: requester.into(),
		agent: agent("reporter"),
		state: "active".into(),
		created_by_task: (requester == "alice").then_some(creating_task),
	};
	let subnode = |node: &str, root: &str| SubnodeRow {
		id: Uuid::new_v4(),
		node: node.into(),
		title: None,
		creator_agent: agent("lead"),
		creator_task: creating_task,
		root_subject: root.into(),
		members: vec![agent("writer")],
	};
	let alice_duty = Uuid::new_v4();
	let rows = Rows {
		nodes: vec![
			NodeRow {
				name: "research".into(),
				label: "Research".into(),
				default: false,
				creator: NodeCreator::Agent {
					root_subject: "alice".into(),
					task_id: creating_task,
					run_id: Uuid::new_v4(),
				},
			},
			NodeRow {
				name: "sales".into(),
				label: "Sales".into(),
				default: true,
				creator: NodeCreator::Person { identity: None },
			},
		],
		duties: vec![
			duty("research", "alice", Some(alice_duty)),
			duty("research", "bob", None),
		],
		subnodes: vec![subnode("research", "alice"), subnode("research", "bob")],
		ledger: Ledger {
			nodes: BTreeMap::from([("research".into(), spend(300)), ("sales".into(), spend(5))]),
			duties: BTreeMap::from([(alice_duty, spend(100))]),
			people: BTreeMap::from([("alice".into(), spend(250)), ("bob".into(), spend(55))]),
		},
		roots: BTreeMap::from([(creating_task, flow_root)]),
	};
	Fixture {
		rows,
		flow_root,
		creating_task,
	}
}

fn viewer(subject: &str) -> Viewer {
	Viewer {
		subject: subject.into(),
		reads: BTreeSet::from(["research".into(), "sales".into()]),
		..Viewer::default()
	}
}

fn since() -> chrono::DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap()
}

#[test]
fn a_member_sees_nodes_and_only_their_own_duties_subnodes_and_spend() {
	// Arrange
	let Fixture {
		rows,
		flow_root,
		creating_task,
	} = fixture();
	// Act
	let chart = shape(&viewer("alice"), rows, since());
	// Assert
	assert_eq!(chart.nodes.len(), 2);
	let research = &chart.nodes[0];
	assert_eq!(research.name, "research");
	assert_eq!(research.flow, Some(flow_root));
	assert_eq!(research.created_by.root_subject(), Some("alice"));
	assert_eq!(research.duties.len(), 1);
	assert_eq!(research.duties[0].requester, "alice");
	assert_eq!(research.duties[0].created_by_task, Some(creating_task));
	assert_eq!(research.duties[0].flow, Some(flow_root));
	assert_eq!(research.duties[0].spend, Some(spend(100)));
	assert_eq!(research.hidden_duties, 1);
	assert_eq!(research.subnodes.len(), 1);
	assert_eq!(research.subnodes[0].root_subject, "alice");
	assert_eq!(research.hidden_subnodes, 1);
	// A member sees no Node's spend, and only their own as a person.
	assert_eq!(research.spend, None);
	assert_eq!(chart.people.len(), 1);
	assert_eq!(chart.people[0].subject, "alice");
	assert_eq!(chart.people[0].spend, spend(250));
}

#[test]
fn a_node_administrator_sees_all_of_their_node() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut carol = viewer("carol");
	carol.administers.insert("research".into());
	// Act
	let chart = shape(&carol, rows, since());
	// Assert
	let research = &chart.nodes[0];
	assert_eq!(research.duties.len(), 2);
	assert_eq!(research.hidden_duties, 0);
	assert_eq!(research.subnodes.len(), 2);
	assert_eq!(research.spend, Some(spend(300)));
	// A Duty with no ledger rows spent nothing.
	assert_eq!(research.duties[1].spend, Some(Spend::default()));
	let sales = &chart.nodes[1];
	assert_eq!(sales.spend, None);
	assert!(chart.people.is_empty());
}

#[test]
fn a_budget_manager_sees_every_spend() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut manager = viewer("dave");
	manager.budget_manager = true;
	// Act
	let chart = shape(&manager, rows, since());
	// Assert
	assert_eq!(chart.nodes[0].spend, Some(spend(300)));
	assert_eq!(chart.nodes[1].spend, Some(spend(5)));
	assert_eq!(chart.people.len(), 2);
	// Spend alone shows no Duty the manager may not otherwise see.
	assert_eq!(chart.nodes[0].hidden_duties, 2);
}

#[test]
fn a_node_without_node_read_is_left_out() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut erin = viewer("erin");
	erin.reads.remove("research");
	erin.administers.insert("research".into());
	// Act
	let chart = shape(&erin, rows, since());
	// Assert
	assert_eq!(
		chart
			.nodes
			.iter()
			.map(|node| node.name.as_str())
			.collect::<Vec<_>>(),
		["sales"]
	);
}

#[test]
fn a_duty_administrator_sees_every_duty_but_no_subnode() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut frank = viewer("frank");
	frank.duty_administrator = true;
	// Act
	let chart = shape(&frank, rows, since());
	// Assert
	assert_eq!(chart.nodes[0].duties.len(), 2);
	assert!(
		chart.nodes[0]
			.duties
			.iter()
			.all(|duty| duty.spend.is_none())
	);
	assert_eq!(chart.nodes[0].hidden_subnodes, 2);
}

#[test]
fn creating_tasks_cover_nodes_duties_and_subnodes() {
	// Arrange
	let Fixture {
		rows,
		creating_task,
		..
	} = fixture();
	// Act
	let tasks = rows.creating_tasks();
	// Assert
	assert_eq!(tasks, vec![creating_task]);
}

#[rstest]
#[case(Utc.with_ymd_and_hms(2026, 11, 19, 13, 5, 0).unwrap(), Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap())]
#[case(Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap())]
#[case(Utc.with_ymd_and_hms(2027, 1, 31, 23, 59, 59).unwrap(), Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap())]
fn spend_counts_from_the_first_of_the_utc_month(
	#[case] now: chrono::DateTime<Utc>,
	#[case] expected: chrono::DateTime<Utc>,
) {
	assert_eq!(month_start(now), expected);
}
