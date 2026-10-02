//! Current readers and the original producer authority must both remain valid.
//! This is a leaf: foreign edges are collected by the caller, never followed here.
use super::*;
use sea_orm::sea_query::{Alias, Asterisk, Expr, LockType, PostgresQueryBuilder, Query};

pub(crate) async fn visible(
	access: &mut Access,
	execution_node: &str,
	id: Uuid,
	admission: Uuid,
) -> Result<bool> {
	let grant: Option<Grant> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::cust(
				"id=$1 AND node_id=$2 AND tenant=$3 AND NOT revoked AND expires_at>CLOCK_TIMESTAMP()",
			))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(execution_node)
	.bind(&access.identity.tenant)
	.fetch_optional(&mut **access.tx)
	.await?;
	let Some(grant) = grant else {
		return Ok(false);
	};
	let Some(bound) = execution::binding(access, id).await? else {
		return Ok(false);
	};
	if bound.admission_id != admission || bound.task_id != grant.task_id {
		return Ok(false);
	}
	peer(access, execution_node).await?;
	// Check source revisions and source policy as the current reader first.
	let task = access.task_read(grant.task_id).await?;
	if task.revision != bound.task_revision {
		return Ok(false);
	}
	if !access.grant_reads_visible(id).await? {
		return Ok(false);
	}
	// Lock the original credential in this same local transaction; do not
	// substitute a newly issued credential with the same subject name.
	let original = grant.identity();
	let snapshot = original.lock_with_mode(&mut access.tx, false).await?;
	let viewer_snapshot = std::mem::replace(&mut access.snapshot, snapshot);
	let viewer_identity = std::mem::replace(&mut access.identity, original);
	let viewer_subjects = std::mem::replace(&mut access.subjects, grant.subject_chain);
	let viewer_context = access.context.clone();
	let cached_runs = std::mem::take(&mut access.cached_runs);
	let cached_humans = std::mem::take(&mut access.cached_humans);
	let result = async {
		let inspection: Inspection = serde_json::from_value(grant.inspection)?;
		source_authority(access, &task, execution_node, &inspection).await?;
		access.remote_semantic_sources(id).await?;
		Ok(true)
	}
	.await;
	access.snapshot = viewer_snapshot;
	access.identity = viewer_identity;
	access.subjects = viewer_subjects;
	access.context = viewer_context;
	access.cached_runs = cached_runs;
	access.cached_humans = cached_humans;
	if matches!(result, Ok(true)) {
		access
			.dependency_frontier
			.as_mut()
			.ok_or(Error::Forbidden)?
			.push(super::super::peer::dependencies::Reference::Admission {
				node_id: execution_node.into(),
				home_node: access.node_id.clone(),
				grant_id: id,
				admission_id: admission,
			});
	}
	result
}

impl Access {
	pub(crate) async fn grant_output_visible(&mut self, id: Uuid) -> Result<bool> {
		let key = (
			self.node_id.clone(),
			id,
			format!("grant:{}", self.authority_context()),
		);
		if !self.checking_reads.insert(key.clone()) {
			return Ok(true);
		}
		let result = self.grant_output_visible_in(id).await;
		self.checking_reads.remove(&key);
		result
	}
	async fn grant_output_visible_in(&mut self, id: Uuid) -> Result<bool> {
		let record: Option<(String, Value)> = sqlx::query_as(
			&Query::select()
				.columns(["node_id", "semantic"].map(Alias::new))
				.from(Alias::new("authorization_remote_grants"))
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut **self.tx)
		.await?;
		let Some((node, semantic)) = record else {
			return Ok(false);
		};
		if serde_json::from_value::<crate::semantic::remote::Binding>(semantic)?.disabled() {
			return self.grant_reads_visible(id).await;
		}
		let Some(bound) = execution::binding(self, id).await? else {
			return Ok(false);
		};
		let coordinator = self.dependency_frontier.is_none();
		if coordinator {
			self.dependency_frontier = Some(vec![]);
		}
		let mut result = Box::pin(visible(self, &node, id, bound.admission_id)).await;
		if coordinator {
			let pending = self.dependency_frontier.take().unwrap_or_default();
			if matches!(result, Ok(true)) {
				result = self.verify_dependencies(pending).await;
			}
		}
		match result {
			Err(
				Error::Forbidden
				| Error::Unauthorized
				| Error::NotFound(_)
				| Error::RemoteSemantic(_),
			) => Ok(false),
			other => other,
		}
	}
}
