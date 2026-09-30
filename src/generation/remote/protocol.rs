//! Bounded allowance RPCs. Receiver leaves never call Home; the Home path is
//! entered only after the execution worker releases its local authority lease.
use super::{
	Purpose, Reserved, Usage,
	dispatch::{self, FinalizeInput, Input},
};
use crate::{
	Error, Result,
	authorization::remote::Description,
	federation::Federation,
	semantic::remote::{Binding, Failure, Operation},
};
use axum::{Json, extract::State, http::HeaderMap};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};

fn exact_provider(description: &Description, usage: &Usage) -> Result<()> {
	match usage.purpose {
		Purpose::Embedding => {
			let Binding::RequiredHome { embedding, .. } = &description.semantic else {
				return Err(Error::RemoteSemantic(Failure::Configuration));
			};
			if **embedding != usage.provider || usage.dispatcher_node != description.source_node {
				return Err(Error::Forbidden);
			}
		}
		Purpose::Compaction => {
			let Binding::RequiredHome {
				compactor: Some(provider),
				..
			} = &description.semantic
			else {
				return Err(Error::RemoteSemantic(Failure::ContextBudget));
			};
			if provider.as_ref() != &usage.provider
				|| usage.dispatcher_node != description.target_node
			{
				return Err(Error::Forbidden);
			}
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "compactor" && d.entry == usage.provider.entry)
				.ok_or(Error::Forbidden)?;
			let config: crate::registry::CompactorConfig =
				serde_json::from_value(definition.metadata.config.clone())?;
			if usage.reserved_tokens < 1025
				|| usage.reserved_tokens > (config.max_request_bytes + 1024) as i64
			{
				return Err(Error::Forbidden);
			}
		}
		Purpose::Inference => {
			let agent: crate::registry::AgentConfig =
				serde_json::from_value(description.inspection.agent.config.clone())?;
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "model" && d.entry == agent.model)
				.ok_or(Error::Forbidden)?;
			if usage.provider.node_id != description.target_node
				|| usage.provider.entry != agent.model
				|| usage.provider.digest != definition.digest
				|| usage.provider.configuration_digest
					!= crate::registry::digest(&definition.metadata.config)
			{
				return Err(Error::Forbidden);
			}
			let config: crate::registry::ModelConfig =
				serde_json::from_value(definition.metadata.config.clone())?;
			if usage.reserved_tokens
				!= (config.context_window + config.output_token_limit() as usize) as i64
			{
				return Err(Error::Forbidden);
			}
		}
	}
	Ok(())
}

pub(crate) async fn reserve(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<Vec<Reserved>>> {
	let source = crate::api::peer_node(&headers)?;
	input.usage.validate()?;
	if source != input.usage.dispatcher_node {
		return Err(Error::Forbidden);
	}
	let (mut access, description) = if input.usage.purpose == Purpose::Embedding {
		crate::authorization::peer::admission::leaf_lease(
			&f,
			source,
			input.usage.grant_id,
			input.usage.admission_id,
		)
		.await?
	} else {
		crate::authorization::remote::description_lease(&f, source, input.usage.grant_id).await?
	};
	let result = async {
		exact_provider(&description, &input.usage)?;
		if input.usage.purpose == Purpose::Embedding {
			let operation: Operation = serde_json::from_value(input.boundary.clone())?;
			operation.validate()?;
			if operation.id != input.usage.operation_id
				|| operation.digest()? != input.usage.input_digest
				|| operation.grant_id != input.usage.grant_id
				|| operation.admission_id != input.usage.admission_id
				|| operation.home_node != source
				|| input.usage.reserved_tokens < 1025
				|| input.usage.reserved_tokens > (operation.query.len() + 1024) as i64
			{
				return Err(Error::Forbidden);
			}
			crate::authorization::peer::semantic::verify_in(&f, &description, &operation).await?;
		} else {
			let bound =
				crate::authorization::remote::execution::binding(&mut access, input.usage.grant_id)
					.await?
					.ok_or(Error::Forbidden)?;
			if bound.admission_id != input.usage.admission_id {
				return Err(Error::Forbidden);
			}
			let verified: bool = crate::authorization::peer::authority_request(
				&f,
				source,
				"/scoped/usage/verify",
				&json!(input),
			)
			.await?;
			if !verified {
				return Err(Error::Forbidden);
			}
		}
		Ok(Json(
			super::reserve(&mut access, &f.store, &input.usage).await?,
		))
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn verify(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<bool>> {
	let source = crate::api::peer_node(&headers)?;
	if input.usage.dispatcher_node != f.config.node_id || input.usage.purpose == Purpose::Embedding
	{
		return Err(Error::Forbidden);
	}
	let (access, description) = crate::authorization::peer::admission::leaf_lease(
		&f,
		source,
		input.usage.grant_id,
		input.usage.admission_id,
	)
	.await?;
	let result=async {
        exact_provider(&description,&input.usage)?;
        let record=dispatch::bound(&f.store,&input).await?;
        let run=f.store.run(input.usage.admission_id).await?;
        if record.peer_node!=source || record.state!="PREPARING" || input.boundary.get("step").and_then(Value::as_i64)!=Some(run.step as i64) {return Err(Error::Forbidden);}
        if !description.semantic.disabled() {
            let ready:bool=sqlx::query_scalar(&Query::select().expr(Expr::cust("COUNT(*) > 0")).from(Alias::new("semantic_remote_operations"))
                .and_where(Expr::cust("admission_id=$1 AND grant_id=$2 AND state='READY' AND (binding->'operation'->'boundary'->>'step')::integer=$3"))
                .to_string(PostgresQueryBuilder)).bind(run.id).bind(input.usage.grant_id).bind(run.step).fetch_one(&f.store.pool).await?;
            if !ready {return Err(Error::RemoteSemantic(Failure::Pending));}
        }
        Ok(Json(true))
    }.await;
	access.finish(result).await
}

pub(crate) async fn finalize(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<FinalizeInput>,
) -> Result<Json<bool>> {
	let source = crate::api::peer_node(&headers)?;
	if input.usage.dispatcher_node != source {
		return Err(Error::Forbidden);
	}
	match super::finalize(&f.store, &input.usage, &input.result).await {
		Ok(()) | Err(Error::RemoteSemantic(Failure::ProviderContract)) => Ok(Json(true)),
		Err(error) => Err(error),
	}
}

pub(crate) struct Reservation {
	pub f: Federation,
	pub input: Input,
}
impl Reservation {
	pub(crate) async fn settle(self, response: &crate::provider::ModelResponse) -> Result<()> {
		let reported = response
			.input_tokens
			.checked_add(response.output_tokens)
			.and_then(|n| i64::try_from(n).ok())
			.filter(|n| response.usage_complete && *n > 0);
		dispatch::finish(
			&self.f,
			&self.input,
			super::Finalization::Settled { reported },
		)
		.await
	}
}
/// The caller has suspended its Access before entering this function.
pub(crate) async fn admit(
	f: &Federation,
	run: &crate::domain::Run,
	attempt: uuid::Uuid,
	purpose: Purpose,
	input_digest: String,
	amount: i64,
) -> Result<Reservation> {
	let description: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("description"))
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::cust("id=$1 AND source_node=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(&run.home_node)
	.fetch_one(&f.store.pool)
	.await?;
	let description: Description = serde_json::from_value(description)?;
	let provider = match purpose {
		Purpose::Inference => {
			let agent: crate::registry::AgentConfig =
				serde_json::from_value(description.inspection.agent.config.clone())?;
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "model" && d.entry == agent.model)
				.ok_or(Error::Forbidden)?;
			crate::semantic::remote::Provider {
				node_id: f.config.node_id.clone(),
				entry: agent.model,
				digest: definition.digest.clone(),
				configuration_digest: crate::registry::digest(&definition.metadata.config),
			}
		}
		Purpose::Compaction => match &description.semantic {
			Binding::RequiredHome {
				compactor: Some(p), ..
			} => (**p).clone(),
			_ => return Err(Error::RemoteSemantic(Failure::ContextBudget)),
		},
		Purpose::Embedding => return Err(Error::Forbidden),
	};
	let input = Input {
		usage: Usage {
			operation_id: attempt,
			attempt_id: attempt,
			dispatcher_node: f.config.node_id.clone(),
			grant_id: description.grant_id,
			admission_id: run.id,
			purpose,
			provider,
			input_digest,
			reserved_tokens: amount,
		},
		boundary: json!({"step":run.step}),
	};
	dispatch::prepare(&f.store, &input, &run.home_node).await?;
	let result: Result<()> = async {
		let mut receipts: Vec<Reserved> = crate::authorization::peer::authority_request(
			f,
			&run.home_node,
			"/scoped/usage/reserve",
			&json!(input),
		)
		.await?;
		if let Binding::RequiredHome { home_lineage, .. } = &description.semantic {
			super::verify_receipts(home_lineage, &receipts, &input.usage)?;
		}
		let (mut access, _) = crate::authorization::peer::admission::worker_lease(f, run)
			.await?
			.ok_or(Error::Forbidden)?;
		let local = super::reserve(&mut access, &f.store, &input.usage).await;
		receipts.extend(access.finish(local).await?);
		dispatch::admitted(&f.store, &input, &receipts).await
	}
	.await;
	if let Err(error) = result {
		let _ = dispatch::finish(f, &input, super::Finalization::Aborted {}).await;
		return Err(error);
	}
	Ok(Reservation {
		f: f.clone(),
		input,
	})
}
