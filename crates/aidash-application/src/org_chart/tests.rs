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
	/// The Thread bob's Duty works in.
	bobs_thread: Uuid,
}

/// Two Nodes: `research`, which alice's chain created, with a Duty of alice and
/// one of bob (each in their own Thread; bob's names carol his successor)
/// and a Subnode of each; and `sales`, which a person created.
fn fixture() -> Fixture {
	let flow_root = Uuid::new_v4();
	let creating_task = Uuid::new_v4();
	let alices_thread = Uuid::new_v4();
	let bobs_thread = Uuid::new_v4();
	let duty = |node: &str, requester: &str, spent: Option<Uuid>| DutyRow {
		id: spent.unwrap_or_else(Uuid::new_v4),
		node: node.into(),
		thread: if requester == "alice" {
			alices_thread
		} else {
			bobs_thread
		},
		origin_thread_id: None,
		successor: (requester == "bob").then(|| "carol".to_owned()),
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
		bobs_thread,
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
fn a_member_sees_only_their_own_duty_and_subnode_and_the_rest_are_counted() {
	// Arrange
	let Fixture {
		rows,
		flow_root,
		creating_task,
		..
	} = fixture();
	// Act
	let chart = shape(&viewer("alice"), rows, since());
	// Assert
	assert_eq!(chart.nodes.len(), 2);
	let research = &chart.nodes[0];
	assert_eq!(research.name, "research");
	assert_eq!(research.flow, Some(flow_root));
	assert_eq!(research.created_by.root_subject(), Some("alice"));
	// bob's Duty and Subnode are hidden from alice, and counted.
	assert_eq!(research.duties.len(), 1);
	assert_eq!(research.duties[0].requester, "alice");
	assert_eq!(research.duties[0].created_by_task, Some(creating_task));
	assert_eq!(research.duties[0].flow, Some(flow_root));
	assert_eq!(research.hidden_duties, 1);
	assert_eq!(research.subnodes.len(), 1);
	assert_eq!(research.subnodes[0].root_subject, "alice");
	assert_eq!(research.hidden_subnodes, 1);
	// Spend: her own Duty's and her own as a person; no Node's.
	assert_eq!(research.duties[0].spend, Some(spend(100)));
	assert_eq!(research.spend, None);
	assert_eq!(chart.people.len(), 1);
	assert_eq!(chart.people[0].subject, "alice");
	assert_eq!(chart.people[0].spend, spend(250));
}

#[test]
fn node_read_alone_shows_no_work_on_the_node() {
	// Arrange: erin requested nothing, created nothing and administers nothing.
	let Fixture { rows, .. } = fixture();
	// Act
	let chart = shape(&viewer("erin"), rows, since());
	// Assert
	let research = &chart.nodes[0];
	assert!(research.duties.is_empty() && research.subnodes.is_empty());
	assert_eq!((research.hidden_duties, research.hidden_subnodes), (2, 2));
	assert!(chart.people.is_empty());
}

#[rstest]
#[case::successor("carol", false, false)]
#[case::thread_reader("erin", true, false)]
#[case::duty_administrator("erin", false, true)]
fn a_duty_shows_to_whoever_its_own_page_shows_to(
	#[case] subject: &str,
	#[case] reads_thread: bool,
	#[case] duty_administrator: bool,
) {
	// Arrange
	let Fixture {
		rows, bobs_thread, ..
	} = fixture();
	let mut person = viewer(subject);
	if reads_thread {
		person.threads.insert(bobs_thread);
	}
	person.duty_administrator = duty_administrator;
	// Act
	let chart = shape(&person, rows, since());
	// Assert: bob's Duty shows; alice's only to the Duty administrator.
	let research = &chart.nodes[0];
	assert!(research.duties.iter().any(|duty| duty.requester == "bob"));
	let expected = if duty_administrator { 2 } else { 1 };
	assert_eq!(research.duties.len(), expected);
	assert_eq!(research.hidden_duties, 2 - expected);
	// Seeing a Duty shows none of its spend beyond the Budget rule.
	assert!(research.duties.iter().all(|duty| duty.spend.is_none()));
	// Neither shows a Subnode.
	assert_eq!(research.hidden_subnodes, 2);
}

#[test]
fn a_node_administrator_sees_its_subnodes_and_spend_but_not_others_duties() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut dave = viewer("dave");
	dave.administers.insert("research".into());
	// Act
	let chart = shape(&dave, rows, since());
	// Assert
	let research = &chart.nodes[0];
	assert_eq!(research.subnodes.len(), 2);
	assert_eq!(research.hidden_subnodes, 0);
	assert_eq!(research.duties.len(), 0);
	assert_eq!(research.hidden_duties, 2);
	assert_eq!(research.spend, Some(spend(300)));
	let sales = &chart.nodes[1];
	assert_eq!(sales.spend, None);
	assert_eq!(sales.hidden_subnodes, 0);
	assert!(chart.people.is_empty());
}

#[test]
fn a_node_administrator_sees_the_spend_of_the_duties_they_see() {
	// Arrange
	let Fixture {
		rows, bobs_thread, ..
	} = fixture();
	let mut dave = viewer("dave");
	dave.administers.insert("research".into());
	dave.threads.insert(bobs_thread);
	// Act
	let chart = shape(&dave, rows, since());
	// Assert: a Duty with no ledger rows spent nothing.
	let research = &chart.nodes[0];
	assert_eq!(research.duties.len(), 1);
	assert_eq!(research.duties[0].spend, Some(Spend::default()));
}

#[test]
fn a_budget_manager_sees_every_spend_but_no_more_work() {
	// Arrange
	let Fixture { rows, .. } = fixture();
	let mut manager = viewer("frank");
	manager.budget_manager = true;
	// Act
	let chart = shape(&manager, rows, since());
	// Assert
	assert_eq!(chart.nodes[0].spend, Some(spend(300)));
	assert_eq!(chart.nodes[1].spend, Some(spend(5)));
	assert_eq!(chart.people.len(), 2);
	assert_eq!(chart.nodes[0].hidden_duties, 2);
	assert_eq!(chart.nodes[0].hidden_subnodes, 2);
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
	// Assert: neither the Node nor its Duties and Subnodes show anywhere.
	assert_eq!(
		chart
			.nodes
			.iter()
			.map(|node| node.name.as_str())
			.collect::<Vec<_>>(),
		["sales"]
	);
	assert!(chart.nodes[0].duties.is_empty() && chart.nodes[0].subnodes.is_empty());
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
