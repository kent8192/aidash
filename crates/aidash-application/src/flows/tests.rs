use super::*;
use rstest::rstest;

/// A Tenant's links, kept in memory.
#[derive(Default)]
struct Graph {
	/// child Task → parent Task
	parent: BTreeMap<Uuid, Uuid>,
	/// Run → its Task
	run_task: BTreeMap<Uuid, Uuid>,
	/// Task → the Run that created it
	origin: BTreeMap<Uuid, Uuid>,
	/// Duty → the Task whose Run set it up
	duty_creator: BTreeMap<Uuid, Uuid>,
	/// Task → the Duty that started it
	duty_task: BTreeMap<Uuid, Uuid>,
	/// How many layers the walk asked for.
	passes: usize,
}

impl Graph {
	fn task(&mut self) -> Uuid {
		Uuid::new_v4()
	}
	fn child(&mut self, parent: Uuid) -> Uuid {
		let task = Uuid::new_v4();
		self.parent.insert(task, parent);
		task
	}
	fn run(&mut self, task: Uuid) -> Uuid {
		let run = Uuid::new_v4();
		self.run_task.insert(run, task);
		run
	}
	fn created_by(&mut self, run: Uuid) -> Uuid {
		let task = Uuid::new_v4();
		self.origin.insert(task, run);
		task
	}
	fn duty(&mut self, creator: Uuid) -> Uuid {
		let duty = Uuid::new_v4();
		self.duty_creator.insert(duty, creator);
		duty
	}
	fn duty_started(&mut self, duty: Uuid) -> Uuid {
		let task = Uuid::new_v4();
		self.duty_task.insert(task, duty);
		task
	}
}

#[async_trait]
impl FlowGraph for Graph {
	async fn children(&mut self, tasks: &[Uuid]) -> Result<Vec<Uuid>> {
		self.passes += 1;
		Ok(self
			.parent
			.iter()
			.filter(|(_, parent)| tasks.contains(parent))
			.map(|(child, _)| *child)
			.collect())
	}
	async fn runs(&mut self, tasks: &[Uuid]) -> Result<Vec<Uuid>> {
		Ok(self
			.run_task
			.iter()
			.filter(|(_, task)| tasks.contains(task))
			.map(|(run, _)| *run)
			.collect())
	}
	async fn created_by_runs(&mut self, runs: &[Uuid]) -> Result<Vec<Uuid>> {
		Ok(self
			.origin
			.iter()
			.filter(|(_, run)| runs.contains(run))
			.map(|(task, _)| *task)
			.collect())
	}
	async fn duties(&mut self, tasks: &[Uuid], _runs: &[Uuid]) -> Result<Vec<Uuid>> {
		Ok(self
			.duty_creator
			.iter()
			.filter(|(_, task)| tasks.contains(task))
			.map(|(duty, _)| *duty)
			.collect())
	}
	async fn duty_tasks(&mut self, duties: &[Uuid]) -> Result<Vec<Uuid>> {
		Ok(self
			.duty_task
			.iter()
			.filter(|(_, duty)| duties.contains(duty))
			.map(|(task, _)| *task)
			.collect())
	}
	async fn parents(&mut self, tasks: &[Uuid]) -> Result<BTreeMap<Uuid, Uuid>> {
		let mut parents = BTreeMap::new();
		for task in tasks {
			let parent = self
				.parent
				.get(task)
				.copied()
				.or_else(|| {
					self.origin
						.get(task)
						.and_then(|run| self.run_task.get(run).copied())
				})
				.or_else(|| {
					self.duty_task
						.get(task)
						.and_then(|duty| self.duty_creator.get(duty).copied())
				});
			if let Some(parent) = parent {
				parents.insert(*task, parent);
			}
		}
		Ok(parents)
	}
}

#[tokio::test]
async fn a_flow_holds_its_tree_what_its_runs_created_and_its_duties_work() {
	// Arrange: a request whose Run delegates a Task, whose Run sets up a Duty.
	let mut graph = Graph::default();
	let root = graph.task();
	let child = graph.child(root);
	let root_run = graph.run(root);
	let delegated = graph.created_by(root_run);
	let delegated_run = graph.run(delegated);
	let duty = graph.duty(delegated);
	let duty_task = graph.duty_started(duty);
	let duty_run = graph.run(duty_task);
	// Another person's request shares nothing with it.
	let other = graph.task();
	let other_run = graph.run(other);
	let other_child = graph.child(other);
	// Act
	let flow = closure(&mut graph, root).await.unwrap();
	// Assert
	assert_eq!(
		flow.tasks,
		BTreeSet::from([root, child, delegated, duty_task])
	);
	assert_eq!(
		flow.runs,
		BTreeSet::from([root_run, delegated_run, duty_run])
	);
	assert_eq!(flow.duties, BTreeSet::from([duty]));
	assert!(!flow.tasks.contains(&other) && !flow.tasks.contains(&other_child));
	assert!(!flow.runs.contains(&other_run));
}

#[tokio::test]
async fn a_cycle_ends_the_walk() {
	// Arrange: a Duty the flow set up started a Task whose Run created a Task
	// under the root again (a link back), and the root's child points back.
	let mut graph = Graph::default();
	let root = graph.task();
	let child = graph.child(root);
	graph.parent.insert(root, child);
	// Act
	let flow = closure(&mut graph, root).await.unwrap();
	// Assert
	assert_eq!(flow.tasks, BTreeSet::from([root, child]));
	assert!(graph.passes <= 3, "{}", graph.passes);
}

#[tokio::test]
async fn every_task_finds_the_root_task_of_its_flow() {
	// Arrange
	let mut graph = Graph::default();
	let root = graph.task();
	let child = graph.child(root);
	let run = graph.run(child);
	let delegated = graph.created_by(run);
	let duty = graph.duty(delegated);
	let duty_task = graph.duty_started(duty);
	let lone = graph.task();
	// Act
	let roots = roots(&mut graph, &[child, delegated, duty_task, lone, root])
		.await
		.unwrap();
	// Assert
	for task in [child, delegated, duty_task, root] {
		assert_eq!(roots[&task], root, "{task}");
	}
	assert_eq!(roots[&lone], lone);
}

#[tokio::test]
async fn a_cycle_upward_ends_where_the_walk_meets_itself() {
	// Arrange
	let mut graph = Graph::default();
	let first = graph.task();
	let second = graph.child(first);
	graph.parent.insert(first, second);
	// Act
	let roots = roots(&mut graph, &[second]).await.unwrap();
	// Assert: the walk stops instead of looping.
	assert!([first, second].contains(&roots[&second]));
}

#[rstest]
#[case::request(false, true)]
#[case::created_under_another(true, false)]
#[tokio::test]
async fn a_root_task_has_nothing_above_it(#[case] under: bool, #[case] root: bool) {
	// Arrange
	let mut graph = Graph::default();
	let top = graph.task();
	let task = if under { graph.child(top) } else { top };
	// Act
	let decided = is_root(&mut graph, task).await.unwrap();
	// Assert
	assert_eq!(decided, root);
}
