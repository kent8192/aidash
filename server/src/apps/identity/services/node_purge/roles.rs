//! Resolve local federation journal roles from verified relational evidence.
//! Home and receiver may be different Nodes of this same Tenant. Never turn
//! recalled source Unit/Entry UUIDs, or an arbitrary home string, into owners.
use super::*;
use aidash_domain::nodes::{NodeAddress, NodeName};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Grant {
	id: Uuid,
	node: String,
}
#[derive(Deserialize)]
struct Admission {
	id: Uuid,
	node: String,
	grant_id: Uuid,
	source_node: String,
}
#[derive(Deserialize)]
struct Run {
	id: Uuid,
	node: String,
	home_node: String,
}
#[derive(Deserialize)]
struct Request {
	id: Uuid,
	node: String,
}
#[derive(Deserialize)]
struct Journal {
	id: Uuid,
	home_node: String,
	grant_id: Uuid,
	admission_id: Uuid,
}
#[derive(Deserialize)]
struct Usage {
	request_id: Uuid,
	attempt_id: Uuid,
	operation_id: Uuid,
	grant_id: Uuid,
	admission_id: Uuid,
}
#[derive(Deserialize)]
struct Embedding {
	workspace_id: Uuid,
	run_id: Option<Uuid>,
}
#[derive(Deserialize)]
struct Dispatch {
	attempt_id: Uuid,
	usage: Value,
}
#[derive(Deserialize)]
struct Boundary {
	operation_id: Uuid,
	grant_id: Uuid,
	admission_id: Uuid,
}
async fn read<T: DeserializeOwned>(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	table: &str,
	columns: &[&str],
) -> Result<Vec<T>> {
	let (sql, values) = Query::select()
		.columns(columns.iter().map(|s| Alias::new(*s)))
		.from(Alias::new(table))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Into::into))
		.collect()
}
fn blocked() -> Error {
	Error::Conflict("Node purge found contradictory local federation journal ownership; repair its Home/receiver evidence before retrying".into())
}
struct Evidence {
	grants: BTreeMap<Uuid, String>,
	admissions: BTreeMap<Uuid, Admission>,
	runs: BTreeMap<Uuid, Run>,
	journals: BTreeMap<Uuid, (Uuid, Uuid, String)>,
}
impl Evidence {
	fn receiver<'a>(
		&'a self,
		admission: Uuid,
		grant: Uuid,
		target: &str,
	) -> Result<Option<&'a str>> {
		let Some(a) = self.admissions.get(&admission) else {
			return Ok(None);
		};
		let run = self.runs.get(&admission);
		if a.grant_id != grant
			|| run.is_none_or(|run| run.node != a.node || run.home_node != a.source_node)
		{
			if a.node == target || run.is_some_and(|run| run.node == target) {
				return Err(blocked());
			}
			return Ok(None);
		}
		Ok(Some(&a.node))
	}
	fn journal(&self, id: Uuid, grant: Uuid, admission: Uuid) -> Option<&str> {
		self.journals
			.get(&id)
			.filter(|(g, a, _)| *g == grant && *a == admission)
			.map(|(_, _, node)| node.as_str())
	}
	// Fallback for content-free usage/dispatch evidence without its primary
	// journal. Disagreeing local roles cannot be resolved by an OR selector.
	fn fallback<'a>(
		&'a self,
		grant: Uuid,
		admission: Uuid,
		target: &str,
	) -> Result<Option<&'a str>> {
		let receiver = self.receiver(admission, grant, target)?;
		let home = self.grants.get(&grant).map(String::as_str);
		match (home, receiver) {
			(Some(a), Some(b)) if a != b => {
				if a == target || b == target {
					Err(blocked())
				} else {
					Ok(None)
				}
			}
			(Some(a), _) => Ok(Some(a)),
			(_, Some(b)) => Ok(Some(b)),
			_ => Ok(None),
		}
	}
}

pub(super) async fn resolve(
	tx: &mut dyn TransactionExecutor,
	store: &Store,
	owners: &mut Owners<'_>,
) -> Result<()> {
	let grants: Vec<Grant> = read(
		tx,
		owners.tenant,
		"authorization_remote_grants",
		&["id", "node"],
	)
	.await?;
	let admissions: Vec<Admission> = read(
		tx,
		owners.tenant,
		"authorization_remote_admissions",
		&["id", "node", "grant_id", "source_node"],
	)
	.await?;
	let runs: Vec<Run> = read(tx, owners.tenant, "runs", &["id", "node", "home_node"]).await?;
	let mut evidence = Evidence {
		grants: grants.into_iter().map(|g| (g.id, g.node)).collect(),
		admissions: admissions.into_iter().map(|a| (a.id, a)).collect(),
		runs: runs.into_iter().map(|r| (r.id, r)).collect(),
		journals: BTreeMap::new(),
	};
	let journals: Vec<Journal> = read(
		tx,
		owners.tenant,
		"semantic_remote_operations",
		&["id", "home_node", "grant_id", "admission_id"],
	)
	.await?;
	for journal in journals {
		let home = evidence.grants.get(&journal.grant_id);
		let receiver = evidence.receiver(journal.admission_id, journal.grant_id, owners.node)?;
		let proven_home = home
			.and_then(|node| NodeName::parse(node).ok())
			.map(|node| store.deployment.node(node));
		let address = NodeAddress::parse(&journal.home_node).ok();
		let owner = if proven_home.is_some() && address == proven_home {
			let home = home.expect("proven local Home");
			if home == owners.node {
				owners.home_journals.push(journal.id);
			}
			Some(home.as_str())
		} else if home.is_some() {
			if home.is_some_and(|node| node == owners.node)
				|| receiver == Some(owners.node)
				|| address.as_ref().is_some_and(|address| {
					address.deployment == store.deployment && address.node.as_str() == owners.node
				}) {
				return Err(blocked());
			}
			None
		} else if let Some(receiver) = receiver {
			let a = &evidence.admissions[&journal.admission_id];
			if NodeAddress::parse(&a.source_node).ok() != address || address.is_none() {
				if receiver == owners.node {
					return Err(blocked());
				} else {
					continue;
				}
			}
			Some(receiver)
		} else {
			None
		};
		let owner = owner.map(str::to_owned);
		if let Some(owner) = owner {
			if owner == owners.node {
				owners.journals.push(journal.id);
			}
			evidence
				.journals
				.insert(journal.id, (journal.grant_id, journal.admission_id, owner));
		}
	}
	let workspaces: Vec<Request> = read(tx, owners.tenant, "workspaces", &["id", "node"]).await?;
	let workspaces: BTreeMap<Uuid, String> =
		workspaces.into_iter().map(|r| (r.id, r.node)).collect();
	let embedding: Vec<Embedding> = read(
		tx,
		owners.tenant,
		"generation_embedding_usage",
		&["workspace_id", "run_id"],
	)
	.await?;
	for usage in embedding {
		if let (Some(workspace), Some(run)) = (
			workspaces.get(&usage.workspace_id),
			usage.run_id.and_then(|id| evidence.runs.get(&id)),
		) && workspace != &run.node
			&& (workspace == owners.node || run.node == owners.node)
		{
			return Err(Error::Conflict("Node purge found conflicting embedding Workspace and Run owners; repair the execution evidence before retrying".into()));
		}
	}
	let requests: Vec<Request> =
		read(tx, owners.tenant, "generation_requests", &["id", "node"]).await?;
	let requests: BTreeMap<Uuid, String> = requests.into_iter().map(|r| (r.id, r.node)).collect();
	let usage: Vec<Usage> = read(
		tx,
		owners.tenant,
		"generation_remote_usage",
		&[
			"request_id",
			"attempt_id",
			"operation_id",
			"grant_id",
			"admission_id",
		],
	)
	.await?;
	for item in usage {
		// The reservation writer locks its local Request/budget: that primary root
		// wins over the other parties' grant/admission roles.
		let owner = if let Some(node) = requests.get(&item.request_id) {
			Some(node.as_str())
		} else if let Some(node) =
			evidence.journal(item.operation_id, item.grant_id, item.admission_id)
		{
			Some(node)
		} else {
			evidence.fallback(item.grant_id, item.admission_id, owners.node)?
		};
		if owner == Some(owners.node) {
			owners.remote_usage.push((item.request_id, item.attempt_id));
		}
	}
	let dispatches: Vec<Dispatch> = read(
		tx,
		owners.tenant,
		"generation_remote_dispatches",
		&["attempt_id", "usage"],
	)
	.await?;
	for dispatch in dispatches {
		let boundary: Boundary = match serde_json::from_value(dispatch.usage) {
			Ok(value) => value,
			Err(_) => continue,
		};
		let owner = if let Some(node) = evidence.journal(
			boundary.operation_id,
			boundary.grant_id,
			boundary.admission_id,
		) {
			Some(node)
		} else {
			evidence.fallback(boundary.grant_id, boundary.admission_id, owners.node)?
		};
		if owner == Some(owners.node) {
			owners.dispatches.push(dispatch.attempt_id);
		}
	}
	Ok(())
}
