//! Native authority scopes retain the physical transaction across application calls.
use super::{Scope, persistence};
use crate::apps::federation::transactions::{
	serializers::authority as dto,
	services::{coordinator, fault, gate},
};
use crate::{
	authorization::access::{Access, NativeAccess},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::transactions::{
		TransactionAuthorityScope,
		authority::{AuthorityRepository, ControlScope, SubmissionScope},
	},
};
use aidash_domain::transactions::{
	Manifest,
	authority::{Binding, Origin, Preflight, Status},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct Repository {
	pub(crate) runtime: Federation,
}

struct Control {
	authority: Scope<Box<Access>>,
	runtime: Federation,
}

struct Submission {
	access: NativeAccess,
	runtime: Federation,
}

impl Repository {
	fn scope(&self, access: Access) -> Box<dyn ControlScope> {
		Box::new(Control {
			authority: Scope {
				access: Box::new(access),
			},
			runtime: self.runtime.clone(),
		})
	}
}

#[async_trait]
impl AuthorityRepository for Repository {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn validate(&self, manifest: &Manifest) -> Result<()> {
		aidash_application::transactions::validate(
			&crate::bootstrap::registry_validation(),
			manifest,
		)
	}
	async fn origin(&self, id: Uuid) -> Result<Option<Origin>> {
		persistence::binding::<dto::Origin>(&self.runtime, "atomic_subjects", id)
			.await
			.map(|origin| origin.map(|origin| (&origin).into()))
			.map_err(Into::into)
	}
	async fn binding(&self, id: Uuid) -> Result<Option<Binding>> {
		persistence::binding::<dto::Binding>(&self.runtime, "atomic_preflights", id)
			.await
			.map(|binding| binding.map(|binding| (&binding).into()))
			.map_err(Into::into)
	}
	async fn source(&self, origin: &Origin) -> Result<Box<dyn ControlScope>> {
		let origin = dto::Origin::from(origin.clone());
		Ok(self.scope(persistence::access(&self.runtime, &origin).await?))
	}
	async fn mapped(&self, input: &Preflight) -> Result<Box<dyn ControlScope>> {
		let input = dto::Preflight::from(input.clone());
		Ok(self.scope(persistence::mapped(&self.runtime, &input).await?))
	}
	async fn remote_preflight(&self, node: &str, input: &Preflight) -> Result<()> {
		let _: Value = coordinator::remote(
			&self.runtime,
			node,
			reqwest::Method::POST,
			"/transactions/preflight",
			Some(&dto::Preflight::from(input.clone())),
		)
		.await?;
		Ok(())
	}
	async fn remote_access(&self, node: &str, input: &Preflight) -> Result<()> {
		let _: Value = coordinator::remote(
			&self.runtime,
			node,
			reqwest::Method::POST,
			"/transactions/access",
			Some(&dto::Preflight::from(input.clone())),
		)
		.await?;
		Ok(())
	}
	async fn remote_ticket(&self, node: &str, id: Uuid) -> Result<Preflight> {
		let input: dto::Preflight = coordinator::remote(
			&self.runtime,
			node,
			reqwest::Method::GET,
			&format!("/transactions/{id}/authority"),
			None::<&()>,
		)
		.await?;
		Ok((&input).into())
	}
	async fn fault(&self, id: Uuid, point: &str) -> Result<()> {
		fault::cut(id, point).await.map_err(Into::into)
	}
	fn wake(&self) {
		self.runtime.notify.notify_waiters();
	}
}

#[async_trait]
impl ControlScope for Control {
	fn authority(&mut self) -> &mut dyn TransactionAuthorityScope {
		&mut self.authority
	}
	fn audit(&mut self, enabled: bool) {
		self.authority.access.audit = enabled;
	}
	async fn gate(&mut self) -> Result<()> {
		gate::read_in(&mut self.authority.access.tx)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn status(&mut self, id: Uuid) -> Result<Status> {
		persistence::status_with(&mut **self.authority.access.tx, id)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn match_origin(&mut self, id: Uuid, origin: &Origin) -> Result<()> {
		persistence::match_origin_with(
			&mut **self.authority.access.tx,
			id,
			Some(&dto::Origin::from(origin.clone())),
		)
		.await
		.map_err(Into::into)
	}
	async fn bind(&mut self, id: Uuid, binding: &Binding) -> Result<()> {
		persistence::bind(
			&mut self.authority.access.tx,
			"atomic_preflights",
			id,
			&json!(dto::Binding::from(binding)),
		)
		.await
		.map_err(Into::into)
	}
	async fn trusted(&mut self, node: &str) -> Result<()> {
		persistence::trusted(&mut self.authority.access, node)
			.await
			.map_err(Into::into)
	}
	async fn insert_attempt(&mut self, id: Uuid, node: &str) -> Result<()> {
		persistence::insert_attempt(&mut self.authority.access, id, node)
			.await
			.map_err(Into::into)
	}
	async fn pending_attempt(&mut self, id: Uuid, node: &str) -> Result<bool> {
		persistence::attempt(&mut self.authority.access, id, node)
			.await
			.map(|attempt| attempt.is_some())
			.map_err(Into::into)
	}
	fn into_submission(self: Box<Self>) -> Result<Box<dyn SubmissionScope>> {
		let Self { authority, runtime } = *self;
		Ok(Box::new(Submission {
			access: (*authority.access).into_native()?,
			runtime,
		}))
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		(*self.authority.access)
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl SubmissionScope for Submission {
	async fn submit(&mut self, manifest: &Manifest, origin: &Origin) -> Result<Status> {
		coordinator::submit_in(
			&self.runtime,
			manifest,
			Some(&dto::Origin::from(origin.clone())),
			self.access.tx.as_mut(),
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	async fn bind(&mut self, id: Uuid, binding: &Binding) -> Result<()> {
		persistence::bind_native(
			self.access.tx.as_mut(),
			"atomic_preflights",
			id,
			&json!(dto::Binding::from(binding)),
		)
		.await
		.map_err(Into::into)
	}
	async fn abort(&mut self, id: Uuid) -> Result<()> {
		coordinator::abort_in(self.access.tx.as_mut(), id)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
