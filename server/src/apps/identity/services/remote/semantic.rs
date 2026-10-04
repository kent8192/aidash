//! Home-side authority for explicitly disclosed remote semantic context.
use super::Inspection;
use crate::semantic::remote::{Operation, Receipt, SourceRead, bounded_query, journal};
use crate::{
	Error, Result,
	authorization::access::Access,
	domain::Task,
	federation::Federation,
	registry::AgentConfig,
	semantic::remote::{Binding, Failure, Request},
};

use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::{json, to_value};
use uuid::Uuid;

pub(crate) async fn binding(
	f: &Federation,
	access: &mut Access,
	task: &Task,
	node: &str,
	inspection: &Inspection,
	request: &Request,
) -> Result<Binding> {
	aidash_application::authorization::source::semantic::binding(
		&mut crate::bootstrap::source_semantic_binding_scope(f, access),
		task,
		node,
		inspection,
		request,
	)
	.await
	.map_err(Into::into)
}

pub(crate) fn failure(error: &Error) -> Failure {
	match error {
		Error::RemoteSemantic(reason) => *reason,
		Error::Forbidden | Error::Unauthorized => Failure::Authority,
		Error::Invalid(_) | Error::NotFound(_) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}

pub(crate) async fn search(
	f: Federation,
	headers: HeaderMap,
	operation: Operation,
) -> Result<Receipt> {
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	operation.validate()?;
	if operation.home_node != f.config.node_id {
		return Err(Error::Forbidden);
	}
	let (mut access, description) = super::description_lease(&f, node, operation.grant_id).await?;
	let result = async {
		if description.semantic.disabled() {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		let bound = super::execution::binding(&mut access, operation.grant_id)
			.await?
			.ok_or(Error::Forbidden)?;
		if bound.admission_id != operation.admission_id {
			return Err(Error::Forbidden);
		}
		access.worker();
		access.durable_audit = true;
		access.read_grant = Some(operation.grant_id);
		let task = access.task_read(description.task.id).await?;
		let mut query = format!("{}\n{}", task.title, task.description);
		for input in &operation.inputs {
			let message: crate::domain::Message = serde_json::from_value(
				access
					.workspace_record(task.workspace_id, "message", input.id)
					.await?,
			)?;
			if crate::semantic::service::content_digest(&message.content) != input.digest {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			query.push('\n');
			query.push_str(&message.content);
		}
		if bounded_query(&query, 32768).0 != operation.query
			|| operation.boundary.inputs_digest != crate::registry::digest(&json!(operation.inputs))
			|| task.revision != operation.boundary.task_revision
		{
			return Err(Error::Forbidden);
		}
		let receiver: bool = crate::authorization::peer::authority_request(
			&f,
			node,
			"/scoped/semantic/verify-operation",
			&to_value(&operation)?,
		)
		.await?;
		if !receiver {
			return Err(Error::Forbidden);
		}
		journal::prepare(&f.store, &operation, &description.semantic).await?;
		let agent: AgentConfig =
			serde_json::from_value(description.inspection.agent.config.clone())?;
		let index =
			crate::semantic::service::index(&mut access.tx, task.workspace_id, false).await?;
		let spec = index.configuration()?;
		let transport_truncated = query.len() > 32768;
		let (query, query_truncated) = bounded_query(&operation.query, spec.max_input_bytes);
		let max_tokens = operation.max_tokens.min(spec.max_result_tokens);
		let input = crate::semantic::Search {
			query,
			agent: Some(crate::domain::qualified_agent(
				node,
				&description.inspection.agent.id,
				&description.inspection.agent.version,
			)),
			metadata: operation.metadata.clone(),
			limit: spec.max_results,
			max_tokens,
		};
		let mut lease = crate::semantic::service::Lease::Inherited(&mut access);
		let prepared = crate::semantic::service::prepare_search(
			&f.store,
			&mut lease,
			task.workspace_id,
			&input,
			Some(&agent),
		)
		.await?;
		let candidate_digest = prepared.candidate_digest();
		let claim = journal::claim(&f.store, operation.id).await?;
		let attempt = match claim {
			journal::Claim::Ready(receipt) if receipt.candidate_digest == candidate_digest => {
				return Ok(*receipt);
			}
			journal::Claim::Ready(_) => {
				journal::expire_cached(&f.store, operation.id).await?;
				match journal::claim(&f.store, operation.id).await? {
					journal::Claim::Attempt(attempt) => attempt,
					journal::Claim::Ready(_) => {
						return Err(Error::RemoteSemantic(Failure::Pending));
					}
				}
			}
			journal::Claim::Attempt(attempt) => attempt,
		};
		let mut dispatch_input = None;
		let result: Result<Receipt> = async {
			let result = if prepared.allowed.is_empty() {
				prepared.result
			} else {
				crate::semantic::service::PreparedSearchExt::check_points(&prepared, &f.store)
					.await?;
				use crate::generation::remote::{self, dispatch};
				let Binding::RequiredHome {
					embedding,
					execution_lineage,
					..
				} = &description.semantic
				else {
					return Err(Error::Forbidden);
				};
				let admission = dispatch::Input {
					usage: remote::Usage {
						operation_id: operation.id,
						attempt_id: attempt.id,
						dispatcher_node: f.config.node_id.clone(),
						grant_id: operation.grant_id,
						admission_id: operation.admission_id,
						purpose: remote::Purpose::Embedding,
						provider: (**embedding).clone(),
						input_digest: operation.digest()?,
						reserved_tokens: (input.query.len() + 1024) as i64,
					},
					boundary: json!(operation),
				};
				dispatch::prepare(&f.store, &admission, node).await?;
				dispatch_input = Some(admission.clone());
				let mut reservations: Vec<remote::Reserved> =
					crate::authorization::peer::authority_request(
						&f,
						node,
						"/scoped/usage/reserve",
						&json!(admission),
					)
					.await?;
				remote::verify_receipts(execution_lineage, &reservations, &admission.usage)?;
				reservations.extend(
					remote::reserve(
						lease.access().ok_or(Error::Forbidden)?,
						&f.store,
						&admission.usage,
					)
					.await?,
				);
				dispatch::admitted(&f.store, &admission, &reservations).await?;
				journal::dispatched(&f.store, &attempt, &json!(reservations)).await?;
				let embedding = crate::semantic::backend::embed(
					&f.store.semantic_client,
					&spec.embedding,
					&input.query,
				)
				.await?;
				dispatch::finish(
					&f,
					&admission,
					remote::Finalization::Settled {
						reported: embedding.tokens.and_then(|n| i64::try_from(n).ok()),
					},
				)
				.await?;
				if embedding
					.tokens
					.is_some_and(|n| n > (input.query.len() + 1024) as u64)
				{
					return Err(Error::RemoteSemantic(Failure::ProviderContract));
				}
				crate::semantic::service::finish_search(
					&f.store,
					&mut lease,
					prepared,
					&embedding.vector,
					true,
				)
				.await?
			};
			let mut receipt = Receipt {
				operation_id: operation.id,
				operation_digest: operation.digest()?,
				home_node: f.config.node_id.clone(),
				tenant: description.source_tenant.clone(),
				workspace_id: task.workspace_id,
				grant_id: operation.grant_id,
				admission_id: operation.admission_id,
				executor: input.agent.clone().ok_or(Error::Forbidden)?,
				binding: description.semantic.clone(),
				retrieved_at: chrono::Utc::now(),
				query_truncated: query_truncated || transport_truncated,
				candidate_digest,
				sources: vec![],
				result,
				estimated_tokens: 0,
			};
			fit_receipt(&mut receipt, max_tokens)?;
			journal::complete(&f.store, &attempt, &receipt).await?;
			Ok(receipt)
		}
		.await;
		match result {
			Ok(receipt) => Ok(receipt),
			Err(error) => {
				let reason = journal::failed(&f.store, &attempt, failure(&error)).await?;
				if let Some(input) = dispatch_input {
					let _ = crate::generation::remote::dispatch::finish(
						&f,
						&input,
						crate::generation::remote::Finalization::Aborted {},
					)
					.await;
				}
				Err(Error::RemoteSemantic(reason))
			}
		}
	}
	.await;
	let receipt = access.finish(result).await?;
	// Release pre-dispatch leases and reacquire after the response. A queued
	// revocation must be observed before source text leaves the Home node.
	let (access, _) = super::description_lease(&f, node, operation.grant_id).await?;
	access.finish(Ok(receipt)).await
}

fn fit_receipt(receipt: &mut Receipt, budget: usize) -> Result<()> {
	let had_matches = !receipt.result.matches.is_empty() || receipt.result.truncated;
	loop {
		receipt.result.estimated_tokens = crate::semantic::service::result_tokens(&receipt.result)?;
		receipt.sources = receipt
			.result
			.matches
			.iter()
			.map(|m| SourceRead {
				entry_id: m.entry_id,
				revision: m.revision,
				content_digest: crate::semantic::service::content_digest(&m.text),
			})
			.collect();
		receipt.estimated_tokens =
			crate::context::estimated_tokens(&serde_json::to_string(receipt)?) + 16;
		if receipt.estimated_tokens <= budget {
			break;
		}
		if receipt.result.matches.pop().is_none() {
			return Err(Error::RemoteSemantic(Failure::ContextBudget));
		}
		receipt.result.truncated = true;
	}
	if had_matches && receipt.result.matches.is_empty() {
		return Err(Error::RemoteSemantic(Failure::ContextBudget));
	}
	Ok(())
}

impl Access {
	pub(crate) async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()> {
		let key = (
			self.node_id.clone(),
			grant,
			format!("semantic:{}", self.authority_context()),
		);
		let Some(_visit) = self.checking_reads.enter(key) else {
			return Ok(());
		};
		self.remote_semantic_sources_in(grant).await
	}
	async fn remote_semantic_sources_in(&mut self, grant: Uuid) -> Result<()> {
		let sources: Vec<(Uuid, i64, String)> = {
			let query_bind_1 = grant;
			sqlx::query_as(
				&Query::select()
					.columns(["entry_id", "revision", "content_digest"].map(Alias::new))
					.from(Alias::new("semantic_remote_reads"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("grant_id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.order_by(Alias::new("entry_id"), reinhardt::query::Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.tx)
			.await?
		};
		let mut lease = crate::semantic::service::Lease::Inherited(self);
		for (id, revision, digest) in sources {
			let entry: crate::semantic::Entry = {
				let query_bind_1 = id;
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("semantic_entries"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **lease.tx())
				.await?
			}
			.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
			if entry.deleted || entry.revision != revision {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			if !lease.permits(&entry, "semantic.read").await? {
				return Err(Error::Forbidden);
			}
			let text = lease
				.source(entry.workspace_id, &serde_json::from_value(entry.source)?)
				.await?
				.ok_or(Error::Forbidden)?;
			if crate::semantic::service::content_digest(&text) != digest {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
		}
		Ok(())
	}
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use http::HeaderMap;

use reinhardt::query::ColumnRef::Asterisk;

use reinhardt::query::SimpleExpr;
