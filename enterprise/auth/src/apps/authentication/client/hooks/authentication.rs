// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Browser workflow. Every mutation uses the latest server-owned CSRF/context.
use super::forms::{CodeForm, CredentialsForm, use_code_form, use_credentials_form};
use crate::apps::authentication::server_fn::*;
use crate::{
	api::{self, Error},
	bridge::Client,
	model::*,
};
use reinhardt::pages::reactive::hooks::{Ref, use_retained_effect};
use reinhardt::pages::{Signal, deps, platform::spawn_task, use_ref, use_state};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Factor {
	Challenge,
	Enroll,
}
#[derive(Clone)]
pub struct State {
	pub loading: bool,
	pub busy: bool,
	pub ready: bool,
	pub signed_out: bool,
	pub context: Option<LoginContext>,
	pub account: Option<Account>,
	pub error: Option<String>,
	pub notice: Option<String>,
	pub factor: Option<Factor>,
	pub enrollment: Option<Enrollment>,
	pub redirect_provider: Option<String>,
}
impl Default for State {
	fn default() -> Self {
		Self {
			loading: true,
			busy: false,
			ready: false,
			signed_out: false,
			context: None,
			account: None,
			error: None,
			notice: None,
			factor: None,
			enrollment: None,
			redirect_provider: None,
		}
	}
}
#[derive(Clone)]
pub enum Action {
	Password,
	Popup(String),
	Redirect(String),
	Register,
	Resend,
	Code,
	Tenant(String),
	Restart,
	Choose(&'static str),
	Link,
	Unlink(String),
	LogoutAll,
	Authenticator(String),
}
pub struct App {
	pub state: Signal<State>,
	pub credentials: CredentialsForm,
	pub code: CodeForm,
	pub account_page: bool,
	flow: Option<String>,
	client: Ref<Option<Client>>,
	generation: Ref<u64>,
}

pub fn use_authentication() -> Rc<App> {
	let location = web_sys::window().expect("browser window").location();
	let flow = web_sys::UrlSearchParams::new_with_str(&location.search().unwrap_or_default())
		.ok()
		.and_then(|params| params.get("flow"))
		.filter(|value| !value.is_empty());
	let app = Rc::new(App {
		state: use_state(State::default()).0,
		credentials: use_credentials_form(),
		code: use_code_form(),
		account_page: location.pathname().unwrap_or_default() == "/settings/account",
		flow,
		client: use_ref(None),
		generation: use_ref(0),
	});
	let mounted = app.clone();
	use_retained_effect(
		move || {
			mounted.reload();
			let mounted = mounted.clone();
			Some(move || {
				// Invalidate outstanding proofs before disposing the SDK. Cleanup must
				// never write a signal belonging to the unmounted Pages scope.
				mounted
					.generation
					.set(mounted.generation.get().wrapping_add(1));
				let client = mounted.client.current_mut().take();
				if let Some(client) = client {
					spawn_task(async move {
						let _ = client.close().await;
					});
				}
			})
		},
		deps![],
	);
	app
}

impl App {
	fn ensure_current(&self, generation: u64) -> Result<(), Error> {
		if self.generation.get() == generation {
			Ok(())
		} else {
			Err(Error::message("This sign-in step has ended."))
		}
	}
	pub fn dispatch(self: &Rc<Self>, action: Action) {
		if self.state.get().busy {
			return;
		}
		self.state.update(|state| {
			state.busy = true;
			state.error = None;
			state.notice = None;
		});
		let app = self.clone();
		let generation = self.generation.get();
		spawn_task(async move {
			if let Err(error) = app.perform(action, generation).await
				&& app.generation.get() == generation
			{
				app.failure(error, generation).await;
			}
			if app.generation.get() == generation {
				app.state.update(|state| state.busy = false);
			}
		});
	}
	fn clear_fields(&self) {
		self.credentials.clear();
		self.code.clear();
	}
	fn client(&self) -> Result<Client, Error> {
		self.client
			.current()
			.clone()
			.ok_or_else(|| Error::message("Start this sign-in step again."))
	}
	async fn close(&self) -> Result<(), Error> {
		let client = self.client.current_mut().take();
		self.state.update(|state| state.ready = false);
		if let Some(client) = client {
			client.close().await?;
		}
		Ok(())
	}
	pub fn reload(self: &Rc<Self>) {
		self.generation.set(self.generation.get() + 1);
		let generation = self.generation.get();
		self.clear_fields();
		self.state.set(State::default());
		let app = self.clone();
		spawn_task(async move {
			let result = async {
				app.close().await?;
				if app.generation.get() != generation {
					return Ok(());
				}
				if app.account_page {
					let account = auth_account().await?;
					if app.generation.get() == generation {
						app.state.update(|state| {
							state.loading = false;
							state.account = Some(account);
						});
					}
					return Ok(());
				}
				let Some(flow) = &app.flow else {
					app.state.update(|state| state.loading = false);
					return Ok(());
				};
				let context = auth_login_context(flow.clone()).await?;
				if app.generation.get() != generation {
					return Ok(());
				}
				app.state.update(|state| {
					state.loading = false;
					state.context = Some(context.clone());
				});
				if context.phase == Phase::ChooseAccount {
					return Ok(());
				}
				let client = Client::new(&context).await?;
				if app.generation.get() != generation {
					client.close().await?;
					return Ok(());
				}
				*app.client.current_mut() = Some(client.clone());
				let result = client.redirect_result().await;
				if app.generation.get() != generation {
					return Ok(());
				}
				app.state.update(|state| state.ready = true);
				if let Some(token) = result? {
					app.state.update(|state| state.busy = true);
					app.complete(token, generation).await?;
				}
				Ok::<_, Error>(())
			}
			.await;
			if app.generation.get() == generation {
				if let Err(error) = result {
					app.failure(error, generation).await;
				}
				if app.generation.get() == generation {
					app.state.update(|state| {
						state.loading = false;
						state.busy = false;
					});
				}
			}
		});
	}
	async fn failure(&self, error: Error, generation: u64) {
		if self.ensure_current(generation).is_err() {
			return;
		}
		match error.code().as_deref() {
			Some("aidash/second-factor-required") => {
				self.credentials.clear_password();
				self.state.update(|state| {
					state.factor = Some(Factor::Challenge);
					state.error = None;
				});
			}
			Some("aidash/enrollment-required") => {
				self.credentials.clear_password();
				self.state.update(|state| {
					state.factor = Some(Factor::Enroll);
					state.error = None;
					state.enrollment = None;
				});
				let enrollment = async { self.client()?.start_enrollment().await }.await;
				if self.ensure_current(generation).is_err() {
					return;
				}
				match enrollment {
					Ok(value) => self.state.update(|state| state.enrollment = Some(value)),
					Err(error) => self.state.update(|state| state.error = Some(error.text())),
				}
			}
			_ => self.state.update(|state| state.error = Some(error.text())),
		}
	}
	fn proof(&self) -> Result<(String, String), Error> {
		let context = self
			.state
			.get()
			.context
			.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
		let flow = self
			.flow
			.clone()
			.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
		Ok((flow, context.csrf))
	}
	fn follow(self: &Rc<Self>, next: Next) -> Result<(), Error> {
		if let Some(target) = next.redirect_to {
			api::navigate(&target)?;
		} else {
			self.reload();
		}
		Ok(())
	}
	async fn complete(self: &Rc<Self>, token: String, generation: u64) -> Result<(), Error> {
		self.ensure_current(generation)?;
		// Close the upstream client before another proof is requested. The token
		// is a transient argument, never a Signal, URL or storage entry.
		self.clear_fields();
		self.state.update(|state| {
			state.factor = None;
			state.enrollment = None;
		});
		self.close().await?;
		self.ensure_current(generation)?;
		let (flow, csrf) = self.proof()?;
		let next = auth_verify(flow, csrf, token).await?;
		self.ensure_current(generation)?;
		self.follow(next)
	}
	async fn perform(self: &Rc<Self>, action: Action, generation: u64) -> Result<(), Error> {
		self.ensure_current(generation)?;
		match action {
			Action::Password => {
				let (email, password) = self.credentials.values();
				let token = self.client()?.password(&email, &password).await?;
				self.complete(token, generation).await?;
			}
			Action::Popup(provider) => {
				self.state.update(|state| state.redirect_provider = None);
				match self.client()?.popup(&provider).await {
					Ok(token) => self.complete(token, generation).await?,
					Err(error) => {
						self.ensure_current(generation)?;
						if error.code().as_deref() == Some("auth/popup-blocked") {
							self.state
								.update(|state| state.redirect_provider = Some(provider));
						}
						return Err(error);
					}
				}
			}
			Action::Redirect(provider) => self.client()?.redirect(&provider).await?,
			Action::Register => {
				let (email, password) = self.credentials.values();
				let sent = self.client()?.register(&email, &password).await?;
				self.ensure_current(generation)?;
				self.credentials.clear_password();
				self.state.update(|state|state.notice=Some(if sent { "Check your email and verify your address, then sign in to continue. This has not created an Aidash Account yet." } else { "Your login was created, but the verification email could not be sent. Enter your password and resend it." }.into()));
			}
			Action::Resend => {
				let (email, password) = self.credentials.values();
				let sent = self.client()?.resend(&email, &password).await?;
				self.ensure_current(generation)?;
				self.credentials.clear_password();
				self.state.update(|state| {
					state.notice = Some(
						if sent {
							"Verification email sent. Verify your address, then sign in."
						} else {
							"This email is already verified. Sign in to continue."
						}
						.into(),
					)
				});
			}
			Action::Code => {
				let client = self.client()?;
				let token = if self.state.get().factor == Some(Factor::Enroll) {
					client.finish_enrollment(&self.code.value()).await?
				} else {
					client.resolve(&self.code.value()).await?
				};
				self.complete(token, generation).await?;
			}
			Action::Tenant(tenant) => {
				self.clear_fields();
				self.close().await?;
				self.ensure_current(generation)?;
				let context = self
					.state
					.get()
					.context
					.ok_or_else(|| Error::message("Start this sign-in step again."))?;
				let flow = self
					.flow
					.clone()
					.ok_or_else(|| Error::message("Start this sign-in step again."))?;
				auth_select_tenant(flow, context.csrf, tenant).await?;
				self.ensure_current(generation)?;
				self.reload();
			}
			Action::Restart => {
				self.clear_fields();
				self.close().await?;
				self.ensure_current(generation)?;
				self.reload();
			}
			Action::Choose(choice) => {
				let (flow, csrf) = self.proof()?;
				let next = auth_choose(flow, csrf, choice.into()).await?;
				self.ensure_current(generation)?;
				self.follow(next)?;
			}
			Action::Link | Action::Unlink(_) => {
				let account = self
					.state
					.get()
					.account
					.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
				let next = match action {
					Action::Link => auth_link(account.csrf).await?,
					Action::Unlink(identity) => auth_unlink(account.csrf, identity).await?,
					_ => unreachable!(),
				};
				self.ensure_current(generation)?;
				api::navigate(
					&next
						.redirect_to
						.ok_or_else(|| Error::message("No verification flow was returned."))?,
				)?;
			}
			Action::LogoutAll => {
				let account = self
					.state
					.get()
					.account
					.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
				let result = auth_logout_all(account.csrf).await?;
				self.ensure_current(generation)?;
				if !result.completed {
					return Err(Error::message("Sign-out was not completed. Try again."));
				}
				self.clear_fields();
				self.state.update(|state| {
					state.signed_out = true;
					state.account = None;
					state.context = None;
				});
			}
			Action::Authenticator(uri) => {
				if !uri.starts_with("otpauth://") {
					return Err(Error::message("Invalid authenticator link"));
				}
				web_sys::window()
					.ok_or_else(|| Error::message("Browser unavailable"))?
					.location()
					.assign(&uri)?;
			}
		}
		Ok(())
	}
}
