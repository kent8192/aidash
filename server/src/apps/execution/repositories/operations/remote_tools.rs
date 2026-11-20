//! Whether a Run executing here for another Home may still use the tool its work depends on
//! (ADR 0139, decision 12). The 30 second Read Recheck holds no Home connection, so it decides
//! from what this Node keeps: its own registration of the Home (Allowance and Block), the
//! Run-only answers on the admission and the Home's Request as the admission last saw it. Every
//! worker boundary describes the grant afresh and saves a changed Request on the admission
//! (`peer_admission::save_tool_request`), so a Request the Home widens or narrows reaches a
//! running command, a Live preview, egress and Subnode members at the Run's next boundary.
use crate::{Error, Result as NativeResult};
use aidash_domain::{
	federation::{
		execution::Description,
		tool_lists::{OwnedRemoteToolLists, ToolList},
	},
	nodes::NodeAddress,
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{QueryRow, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// The Run whose lists bound `run`'s work. A Subnode member's Run uses at most what the Run
/// that created its Subnode may use, so it is bounded by that creator, and by the creator's own
/// creator when that one is a member too. Any other Run is its own. Subnodes nest without a
/// depth limit (#1012, ADR 0143), so the walk goes all the way up: stopping part way would hand
/// a deep member a Run of this Tenant with no lists, which would bound nothing. A Run whose
/// own Thread is a Subnode it created, which the schema allows, bounds itself. Any longer
/// cycle, which creation never makes, is refused: ending the walk on it would hand back a
/// member Run whose lists are none, and skip the lists of a Run in the cycle that has them.
pub(crate) async fn bounding_run(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> NativeResult<Uuid> {
	let mut current = run;
	let mut seen = std::collections::HashSet::from([run]);
	loop {
		let (sql, values) = Query::select()
			.column(Alias::new("creator_run_id"))
			.from(Alias::new("subnodes"))
			.and_where(
				Expr::col("thread_id").in_subquery(
					Query::select()
						.column(Alias::new("thread_id"))
						.from(Alias::new("task_executions"))
						.and_where(Expr::col("id").eq(Expr::value(current)))
						.to_owned(),
				),
			)
			.build(PostgresQueryBuilder);
		let creator: Option<Uuid> = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.and_then(|row| QueryRow::from_backend_row(row).get("creator_run_id"));
		match creator {
			None => break,
			Some(creator) if creator == current => break,
			Some(creator) if seen.insert(creator) => current = creator,
			Some(_) => return Err(Error::Forbidden),
		}
	}
	Ok(current)
}

/// Whether `run` is a Subnode member working for a Run that executes for another Home. Its
/// requests to a person cannot reach that Home, which knows only the creating Run, so they are
/// refused instead of going to a person of this Tenant (ADR 0110, ADR 0139).
pub(crate) async fn is_remote_member(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> NativeResult<bool> {
	let bound = bounding_run(&mut *tx, run).await?;
	Ok(bound != run && is_admitted(tx, bound).await?)
}

/// Whether this Node executes `run` for another Home, that is, holds an admission for it.
pub(crate) async fn is_admitted(tx: &mut dyn TransactionExecutor, run: Uuid) -> NativeResult<bool> {
	Ok(
		crate::apps::federation::peer::repositories::admissions::for_run(tx, run)
			.await?
			.is_some(),
	)
}

/// The lists that decide `run`'s tools, from what this Node holds. `None` when the Run has no
/// admission here, or its Home shares this Tenant and no lists apply. A Home that asks for
/// tools but that this Node holds no registration of is refused.
pub(crate) async fn lists(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> NativeResult<Option<OwnedRemoteToolLists>> {
	let Some(admission) =
		crate::apps::federation::peer::repositories::admissions::for_run(&mut *tx, run).await?
	else {
		return Ok(None);
	};
	let description: Description = serde_json::from_value(admission.description.0.clone())?;
	let request =
		aidash_application::authorization::peer::tool_request::requested_tools(&description)
			.map_err(Error::from)?;
	let home = NodeAddress::parse(&admission.source_node).map_err(|_| Error::Forbidden)?;
	let registration = crate::apps::identity::repositories::node_registrations::get(
		&mut *tx,
		&admission.tenant,
		&admission.node,
		&home,
	)
	.await?;
	let Some(registration) = registration else {
		return match request {
			None => Ok(None),
			Some(_) => Err(Error::Forbidden),
		};
	};
	let run_allowance: ToolList = match admission.run_tool_allowance {
		Some(stored) => serde_json::from_value(stored.0)?,
		None => ToolList::default(),
	};
	Ok(Some(OwnedRemoteToolLists {
		request: request.map(|request| request.tools).unwrap_or_default(),
		allowance: registration.tool_allowance,
		block: registration.tool_block,
		// The Home's Run-only answers already travel inside its Request.
		run_request: ToolList::default(),
		run_allowance,
	}))
}

/// Whether the work `run` has going with `tool` must stop. A Run of this Node's own Home is
/// never stopped by lists; a Subnode member's Run answers to the lists of the Run that created
/// its Subnode.
pub(crate) async fn withdrawn(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	tool: &str,
) -> NativeResult<bool> {
	let bound = bounding_run(&mut *tx, run).await?;
	let lists = lists(tx, bound).await?;
	Ok(aidash_application::authorization::tools::tool_withdrawn(
		lists.as_ref(),
		tool,
	))
}

/// The egress hosts the Home of the Run executing here lets its commands reach (ADR 0139,
/// decision 21), to be met besides the Scratch Space's own list.
///
/// This is the Home's Request as the admission last saw it: the egress proxy has no Federation
/// and holds no Home connection, so a Request the Home narrows reaches a running command only
/// once the Run's next worker boundary has described the grant and saved it on the admission. The 30 second Read Recheck
/// stops a command whose tool a list withdrew, but not one that merely narrows the hosts.
///
/// `Ok(None)` puts no further limit: a Run no other Home started, or one whose Home shares this
/// Tenant (its Description carries no tool authorization detail). A Run another Home started
/// whose admission cannot be read (its Run row exists here) is refused, never left on the Space's hosts alone. A Subnode
/// member's Run is bounded by the hosts of the Run that created its Subnode.
pub(crate) async fn home_hosts(
	tx: &mut dyn TransactionExecutor,
	deployment: &str,
	run: Uuid,
) -> NativeResult<Option<Vec<aidash_domain::federation::tool_lists::HostPattern>>> {
	let bound = bounding_run(&mut *tx, run).await?;
	let Some(admission) =
		crate::apps::federation::peer::repositories::admissions::for_run(&mut *tx, bound).await?
	else {
		// A Run with no row is no Run another Home started, since such a Run always has one.
		let remote = crate::apps::execution::models::Run::read_in(&mut *tx, bound, None)
			.await?
			.is_some_and(|row| !row.is_local(deployment));
		return if remote {
			Err(Error::Forbidden)
		} else {
			Ok(None)
		};
	};
	let description: Description = serde_json::from_value(admission.description.0.clone())?;
	Ok(
		aidash_application::authorization::peer::tool_request::requested_tools(&description)
			.map_err(Error::from)?
			.map(|request| request.hosts),
	)
}

#[cfg(test)]
mod tests {
	use crate::apps::execution::test_database::{DatabaseFixture, database, fixture_scratch_space};
	use rstest::rstest;
	use uuid::Uuid;

	/// A Run in a Thread of its own.
	async fn run(driver: &sqlx::PgPool, node: &str) -> (Uuid, Uuid) {
		let scratch = fixture_scratch_space(driver, "acme", node, "reader").await;
		let (thread, task, run) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
		sqlx::query("INSERT INTO threads (id,tenant,node,title,revision,scratch_space_id,created_by) VALUES ($1,'acme',$2,'Fixture',1,$3,'reader')").bind(thread).bind(node).bind(scratch).execute(driver).await.unwrap();
		sqlx::query("INSERT INTO tasks (id,tenant,thread_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Fixture','Fixture','COMPLETED',1,'reader')").bind(task).bind(thread).execute(driver).await.unwrap();
		let pending = aidash_domain::run_state::encode(
			&aidash_domain::run_state::RunState::Completed(
				aidash_domain::run_state::TerminalState {},
			),
			&aidash_domain::run_state::RecoveryState::default(),
		)
		.unwrap();
		sqlx::query("INSERT INTO task_executions (id,tenant,task_id,thread_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,'aidash://origin-fixture','fixture','1.0.0','COMPLETED',$5,1)").bind(run).bind(task).bind(thread).bind(node).bind(pending).execute(driver).await.unwrap();
		(run, thread)
	}

	/// Makes `thread` a Subnode that the Run `creator` created.
	async fn subnode(driver: &sqlx::PgPool, thread: Uuid, creator: Uuid) {
		let id = Uuid::new_v4();
		sqlx::query(
			"INSERT INTO subnodes (id, tenant, node, creator_run_id, creator_task_id, \
			  creator_agent_id, creator_agent_version, leader_agent_id, leader_agent_version, \
			  allowance, thread_id, title, state, created_at, creation_key, creation_fingerprint) \
			 SELECT $1, tenant, node, id, task_id, agent_id, agent_version, agent_id, agent_version, \
			  $2, $3, 'Cycle', 'active', now(), $1::text, $1::text \
			 FROM task_executions WHERE id = $4",
		)
		.bind(id)
		.bind(serde_json::json!({"tools":[],"secrets":[]}))
		.bind(thread)
		.bind(creator)
		.execute(driver)
		.await
		.unwrap();
	}

	/// The walk up a member's creators refuses a cycle, which would otherwise
	/// end on a member Run with no lists; a Run whose own Thread is a Subnode
	/// it created still bounds itself.
	#[rstest]
	#[tokio::test]
	async fn a_cycle_of_subnode_creators_is_refused(#[future] database: DatabaseFixture) {
		// Arrange
		let database = database.await;
		let scope = database.tenant("acme").await;
		let pool = database.pool().await;
		let driver = pool.fixture_driver().clone();
		let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
		let (own, own_thread) = run(&driver, &node).await;
		subnode(&driver, own_thread, own).await;
		let (first, first_thread) = run(&driver, &node).await;
		let (second, second_thread) = run(&driver, &node).await;
		subnode(&driver, first_thread, second).await;
		subnode(&driver, second_thread, first).await;

		// Act
		let mut tx = database.begin_in(&scope).await;
		let itself = super::bounding_run(&mut *tx, own).await;
		let cycle = [
			super::bounding_run(&mut *tx, first).await,
			super::bounding_run(&mut *tx, second).await,
		];
		tx.rollback().await.unwrap();

		// Assert
		assert_eq!(itself.unwrap(), own);
		for refused in cycle {
			assert!(
				matches!(refused, Err(crate::Error::Forbidden)),
				"{refused:?}"
			);
		}
	}
}
