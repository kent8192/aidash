//! Each native Home model attempt reserves both origin lineages before provider I/O.
use super::memory_models::Models;
use crate::{Error, Result, federation::Federation};
use aidash_domain::generation::{dispatch::Input, remote::*};
use aidash_domain::{
	registry::EntityRef,
	semantic::remote::{NativeBinding, Operation},
};

#[derive(Clone)]
pub(crate) struct Origin {
	pub runtime: Federation,
	pub receiver: String,
	pub operation: Operation,
	pub binding: NativeBinding,
}

pub(crate) struct Reservation {
	runtime: Federation,
	input: Input,
}
impl Reservation {
	pub(crate) async fn settle(self, reported: Option<u64>) -> Result<()> {
		aidash_application::generation::dispatch::finish(
			&crate::bootstrap::generation_dispatch_repository(&self.runtime.store),
			&crate::bootstrap::generation_dispatch_settlement(&self.runtime),
			&self.input,
			Finalization::Settled {
				reported: reported.and_then(|n| i64::try_from(n).ok()),
			},
		)
		.await
		.map_err(Into::into)
	}
}

pub(crate) async fn reserve(
	models: &Models,
	origin: &Origin,
	reference: &EntityRef,
	attempt: uuid::Uuid,
	amount: usize,
) -> Result<Reservation> {
	let role = origin
		.binding
		.banks
		.iter()
		.find(|bank| bank.bank == models.bank && bank.provider.entry == models.provider)
		.and_then(|bank| {
			bank.roles
				.iter()
				.find(|provider| provider.entry == *reference)
		})
		.ok_or(Error::Forbidden)?;
	let input = Input {
		usage: Usage {
			operation_id: origin.operation.id,
			attempt_id: attempt,
			dispatcher_node: origin.runtime.config.node_id.clone(),
			grant_id: origin.operation.grant_id,
			admission_id: origin.operation.admission_id,
			purpose: Purpose::Memory,
			provider: role.clone(),
			input_digest: origin.operation.digest()?,
			reserved_tokens: i64::try_from(amount).map_err(|_| Error::SemanticUnavailable)?,
		},
		boundary: serde_json::to_value(&origin.operation)?,
	};
	let dispatch = crate::bootstrap::generation_dispatch_repository(&origin.runtime.store);
	let settlement = crate::bootstrap::generation_dispatch_settlement(&origin.runtime);
	aidash_application::generation::dispatch::prepare(&dispatch, &input, &origin.receiver).await?;
	let result: Result<()> = async {
		let mut receipts: Vec<Reserved> = crate::authorization::peer::authority_request(
			&origin.runtime,
			&origin.receiver,
			"/scoped/usage/reserve",
			&serde_json::to_value(&input)?,
		)
		.await?;
		let (mut access, description) = crate::authorization::remote::description_lease(
			&origin.runtime,
			&origin.receiver,
			origin.operation.grant_id,
		)
		.await?;
		let aidash_domain::semantic::remote::Binding::RequiredHome {
			execution_lineage,
			home_lineage,
			..
		} = &description.semantic
		else {
			return Err(Error::Forbidden);
		};
		verify_receipts(execution_lineage, &receipts, &input.usage)?;
		let home =
			crate::generation::remote::reserve(&mut access, &origin.runtime.store, &input.usage)
				.await;
		let home = access.finish(home).await?;
		verify_receipts(home_lineage, &home, &input.usage)?;
		receipts.extend(home);
		aidash_application::generation::dispatch::admitted(&dispatch, &input, &receipts).await?;
		Ok(())
	}
	.await;
	if let Err(error) = result {
		let _ = aidash_application::generation::dispatch::finish(
			&dispatch,
			&settlement,
			&input,
			Finalization::Aborted {},
		)
		.await;
		return Err(error);
	}
	Ok(Reservation {
		runtime: origin.runtime.clone(),
		input,
	})
}
