use super::*;

fn id(n: u128) -> Uuid {
	Uuid::from_u128(n)
}

fn child(n: u128, status: TaskStatus) -> PendingChild {
	PendingChild {
		id: id(n),
		title: format!("Task {n}"),
		status,
		dependencies: Vec::new(),
		abandonable: true,
		started: false,
	}
}

fn started(mut child: PendingChild) -> PendingChild {
	child.started = true;
	child
}

fn after(mut child: PendingChild, dependency: u128) -> PendingChild {
	child.dependencies.push(id(dependency));
	child
}

fn ids(tasks: &[UnassignedTask]) -> Vec<Uuid> {
	tasks.iter().map(|task| task.id).collect()
}

#[test]
fn an_open_task_nobody_started_is_unassigned() {
	let found = classify(&[child(1, TaskStatus::Open)]);
	assert_eq!(found.len(), 1);
	assert_eq!(found[0].id, id(1));
	assert_eq!(found[0].title, "Task 1");
	assert!(found[0].abandonable && !found[0].handed_over);
}

#[test]
fn an_open_task_somebody_started_is_not() {
	// A Run under way, another Node, or a generated specialist awaiting
	// approval.
	assert!(classify(&[started(child(1, TaskStatus::Open))]).is_empty());
}

#[test]
fn running_and_failed_tasks_are_never_unassigned() {
	for status in [
		TaskStatus::Running,
		TaskStatus::Failed,
		TaskStatus::Blocked,
		TaskStatus::Cancelled,
	] {
		assert!(classify(&[child(1, status)]).is_empty(), "{status:?}");
	}
}

#[test]
fn a_claimed_task_alone_is_not_unassigned() {
	assert!(classify(&[child(1, TaskStatus::Claimed)]).is_empty());
}

#[test]
fn a_task_waiting_on_an_unassigned_sibling_is_handed_over() {
	// A locally delegated child is CLAIMED while it waits on dependencies.
	let found = classify(&[
		child(1, TaskStatus::Open),
		after(child(2, TaskStatus::Claimed), 1),
		after(started(child(3, TaskStatus::Open)), 1),
	]);
	assert_eq!(ids(&found), [id(1), id(2), id(3)]);
	assert!(!found[0].handed_over);
	assert!(found[1].handed_over && found[2].handed_over);
}

#[test]
fn a_task_waiting_on_a_running_sibling_is_not_unassigned() {
	let found = classify(&[
		child(1, TaskStatus::Running),
		after(child(2, TaskStatus::Claimed), 1),
	]);
	assert!(found.is_empty());
}

#[test]
fn a_chain_of_siblings_is_covered_whatever_the_order() {
	// 4 waits on 3 waits on 2 waits on 1; ids sort the other way round.
	let found = classify(&[
		after(child(1, TaskStatus::Claimed), 2),
		after(child(2, TaskStatus::Claimed), 3),
		after(child(3, TaskStatus::Claimed), 4),
		child(4, TaskStatus::Open),
	]);
	assert_eq!(ids(&found), [id(1), id(2), id(3), id(4)]);
	assert_eq!(
		found
			.iter()
			.map(|task| task.handed_over)
			.collect::<Vec<_>>(),
		[true, true, true, false]
	);
}

#[test]
fn a_dependency_outside_the_children_never_makes_a_child_unassigned() {
	let found = classify(&[after(child(1, TaskStatus::Claimed), 99)]);
	assert!(found.is_empty());
	let found = classify(&[after(started(child(1, TaskStatus::Open)), 99)]);
	assert!(found.is_empty());
}

#[test]
fn a_cycle_of_waiting_tasks_stays_out() {
	let found = classify(&[
		after(child(1, TaskStatus::Claimed), 2),
		after(child(2, TaskStatus::Claimed), 1),
	]);
	assert!(found.is_empty());
}

#[test]
fn the_flags_come_from_the_input() {
	let mut person = child(1, TaskStatus::Open);
	person.abandonable = false;
	let found = classify(&[person]);
	assert!(!found[0].abandonable);
}

fn unassigned(n: u128, abandonable: bool, handed_over: bool) -> UnassignedTask {
	UnassignedTask {
		id: id(n),
		title: format!("Task {n}"),
		abandonable,
		handed_over,
	}
}

fn summary(open: u64, failed: bool, unassigned: Vec<UnassignedTask>) -> ChildTaskSummary {
	ChildTaskSummary {
		has_pending: open > 0,
		has_failed: failed,
		open_tasks: open,
		unassigned,
		terminal_fingerprint: [0; 16],
	}
}

fn none() -> BTreeSet<Uuid> {
	BTreeSet::new()
}

#[test]
fn nothing_pending_finishes() {
	assert_eq!(
		decide_children(&summary(0, false, vec![]), &none(), false),
		ChildrenDecision::Finish
	);
}

#[test]
fn a_failed_child_goes_to_a_person_first() {
	let summary = summary(2, true, vec![unassigned(1, true, false)]);
	assert_eq!(
		decide_children(&summary, &none(), false),
		ChildrenDecision::AskAboutFailed
	);
}

#[test]
fn another_child_that_can_change_means_waiting() {
	let summary = summary(2, false, vec![unassigned(1, true, false)]);
	assert_eq!(
		decide_children(&summary, &none(), false),
		ChildrenDecision::Wait
	);
	assert_eq!(
		decide_children(&summary, &none(), true),
		ChildrenDecision::Wait
	);
}

#[test]
fn an_untold_task_is_told_once() {
	let summary = summary(
		2,
		false,
		vec![unassigned(1, true, false), unassigned(2, true, false)],
	);
	let told = BTreeSet::from([id(1)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::Tell(vec![unassigned(2, true, false)])
	);
}

#[test]
fn a_notice_names_at_most_32_tasks_in_id_order() {
	let tasks: Vec<_> = (1..=40).rev().map(|n| unassigned(n, true, false)).collect();
	let mut tasks = tasks;
	tasks.sort_by_key(|task| task.id);
	let summary = summary(40, false, tasks);
	let ChildrenDecision::Tell(told) = decide_children(&summary, &none(), false) else {
		panic!("expected a notice");
	};
	assert_eq!(told.len(), LIST_LIMIT);
	assert_eq!(told[0].id, id(1));
	assert_eq!(told[31].id, id(32));
}

#[test]
fn with_no_step_left_nobody_is_told() {
	let summary = summary(1, false, vec![unassigned(1, true, false)]);
	assert_eq!(
		decide_children(&summary, &none(), true),
		ChildrenDecision::FinishAbandoning(vec![unassigned(1, true, false)])
	);
}

#[test]
fn told_tasks_made_by_this_run_are_abandoned_as_the_parent_completes() {
	let summary = summary(1, false, vec![unassigned(1, true, false)]);
	let told = BTreeSet::from([id(1)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::FinishAbandoning(vec![unassigned(1, true, false)])
	);
}

#[test]
fn a_task_made_elsewhere_goes_to_a_person_and_nothing_is_abandoned() {
	let summary = summary(
		2,
		false,
		vec![unassigned(1, true, false), unassigned(2, false, false)],
	);
	let told = BTreeSet::from([id(1), id(2)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::AskAboutUnassigned(vec![unassigned(2, false, false)])
	);
}

#[test]
fn a_task_with_open_children_of_its_own_is_not_abandonable_so_it_goes_to_a_person() {
	let summary = summary(1, false, vec![unassigned(1, false, false)]);
	let told = BTreeSet::from([id(1)]);
	assert_eq!(
		decide_children(&summary, &told, true),
		ChildrenDecision::AskAboutUnassigned(vec![unassigned(1, false, false)])
	);
}

#[test]
fn a_handed_over_child_makes_the_parent_abandon_and_wait() {
	let summary = summary(
		2,
		false,
		vec![unassigned(1, true, false), unassigned(2, true, true)],
	);
	let told = BTreeSet::from([id(1), id(2)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::AbandonAndWait(vec![unassigned(1, true, false)])
	);
}

#[test]
fn a_handed_over_child_whose_root_goes_to_a_person_waits_for_the_person() {
	let summary = summary(
		2,
		false,
		vec![unassigned(1, false, false), unassigned(2, true, true)],
	);
	let told = BTreeSet::from([id(1), id(2)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::AskAboutUnassigned(vec![unassigned(1, false, false)])
	);
}

#[test]
fn nothing_to_abandon_beside_a_handed_over_child_waits() {
	// Not reachable from real data: the root of a handed-over chain is
	// either abandonable or asked about.
	let summary = summary(1, false, vec![unassigned(2, true, true)]);
	let told = BTreeSet::from([id(2)]);
	assert_eq!(
		decide_children(&summary, &told, false),
		ChildrenDecision::Wait
	);
}

#[test]
fn the_sentence_follows_the_answer_and_names_each_task() {
	let text = answer_with_abandoned(
		"Done.",
		&[unassigned(1, true, false), unassigned(2, true, false)],
	);
	assert!(text.starts_with("Done.\n\n"));
	assert!(text.contains("\"Task 1\""));
	assert!(text.contains("\"Task 2\""));
	assert!(!text.contains("more"));
}

#[test]
fn the_sentence_names_32_tasks_then_counts_the_rest() {
	let many: Vec<_> = (1..=40).map(|n| unassigned(n, true, false)).collect();
	let text = answer_with_abandoned("Done.", &many);
	assert!(text.contains("\"Task 32\""));
	assert!(!text.contains("\"Task 33\""));
	assert!(text.contains("and 8 more"));
}

#[test]
fn an_empty_list_leaves_the_answer_alone() {
	assert_eq!(answer_with_abandoned("Done.", &[]), "Done.");
}

#[test]
fn the_request_names_the_tasks_by_title() {
	let text = request_prompt(&[unassigned(1, false, false), unassigned(2, false, false)]);
	assert!(text.contains("\"Task 1\"") && text.contains("\"Task 2\""));
	assert!(text.contains("hand"));
	assert!(text.contains("abandon"));
}

#[test]
fn an_old_summary_without_the_field_reads_as_empty() {
	let mut value = serde_json::to_value(summary(0, false, vec![])).unwrap();
	value.as_object_mut().unwrap().remove("unassigned");
	let read: ChildTaskSummary = serde_json::from_value(value).unwrap();
	assert!(read.unassigned.is_empty());
}
