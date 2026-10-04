//! Authenticate wire inputs and compose the portable allowance use cases.
use super::{
	Purpose, Reserved,
	dispatch::{self, FinalizeInput, Input},
};
use crate::{Result, federation::Federation};
use aidash_application::generation::protocol;
use http::HeaderMap;

pub(crate) async fn reserve(
	f: Federation,
	headers: HeaderMap,
	input: Input,
) -> Result<Vec<Reserved>> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	protocol::reserve(
		&crate::bootstrap::generation_protocol_authority(&f),
		source,
		input,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn verify(f: Federation, headers: HeaderMap, input: Input) -> Result<bool> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	protocol::verify(
		&crate::bootstrap::generation_protocol_authority(&f),
		&crate::bootstrap::generation_protocol_repository(&f.store),
		&crate::bootstrap::generation_dispatch_repository(&f.store),
		source,
		input,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn finalize(
	f: Federation,
	headers: HeaderMap,
	input: FinalizeInput,
) -> Result<bool> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	protocol::finalize(
		&crate::bootstrap::generation_settlement_repository(&f.store),
		source,
		input,
	)
	.await
	.map_err(Into::into)
}

pub(crate) struct Reservation {
	pub f: Federation,
	pub input: Input,
}
impl Reservation {
	pub(crate) async fn settle(self, response: &crate::provider::ModelResponse) -> Result<()> {
		let reported = aidash_domain::generation::inference::complete_reported_usage(response);
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
	let input = protocol::admit(
		&crate::bootstrap::generation_protocol_authority(f),
		&crate::bootstrap::generation_protocol_repository(&f.store),
		&crate::bootstrap::generation_dispatch_repository(&f.store),
		&crate::bootstrap::generation_dispatch_settlement(f),
		protocol::Admission {
			run,
			attempt,
			purpose,
			input_digest,
			amount,
		},
	)
	.await?;
	Ok(Reservation {
		f: f.clone(),
		input,
	})
}
