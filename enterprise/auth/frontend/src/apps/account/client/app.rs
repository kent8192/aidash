//! Browser workflow. Every mutation uses the latest server-owned CSRF/context.
use crate::{
	api::{self, Error},
	bridge::Client,
	model::*,
};
use reinhardt::pages::{Page, Signal, platform::spawn_task};
use serde_json::{Value, json};
use std::{
	cell::{Cell, RefCell},
	rc::Rc,
};

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
	AccountStart(&'static str, Option<String>),
	LogoutAll,
	Authenticator(String),
}
pub struct App {
	pub state: Signal<State>,
	pub email: Signal<String>,
	pub password: Signal<String>,
	pub code: Signal<String>,
	pub account_page: bool,
	flow: Option<String>,
	client: RefCell<Option<Client>>,
	generation: Cell<u64>,
}

pub fn page() -> Page {
	let location = web_sys::window().expect("browser window").location();
	let flow = web_sys::UrlSearchParams::new_with_str(&location.search().unwrap_or_default())
		.ok()
		.and_then(|params| params.get("flow"))
		.filter(|value| !value.is_empty());
	let app = Rc::new(App {
		state: Signal::new(State::default()),
		email: Signal::new(String::new()),
		password: Signal::new(String::new()),
		code: Signal::new(String::new()),
		account_page: location.pathname().unwrap_or_default() == "/settings/account",
		flow,
		client: RefCell::new(None),
		generation: Cell::new(0),
	});
	app.reload();
	Page::reactive(move || crate::view::frame(crate::view::content(&app, &app.state.get())))
}

impl App {
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
			if let Err(error) = app.perform(action).await
				&& app.generation.get() == generation
			{
				app.failure(error).await;
			}
			if app.generation.get() == generation {
				app.state.update(|state| state.busy = false);
			}
		});
	}
	fn clear_fields(&self) {
		self.email.set(String::new());
		self.password.set(String::new());
		self.code.set(String::new());
	}
	fn client(&self) -> Result<Client, Error> {
		self.client
			.borrow()
			.clone()
			.ok_or_else(|| Error::message("Start this sign-in step again."))
	}
	async fn close(&self) -> Result<(), Error> {
		let client = self.client.borrow_mut().take();
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
				if app.account_page {
					let account = api::request("/account", None).await?;
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
				let query = web_sys::UrlSearchParams::new()?;
				query.append("flow", flow);
				let context: LoginContext = api::request(
					&format!(
						"/login/context?{}",
						query.to_string().as_string().unwrap_or_default()
					),
					None,
				)
				.await?;
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
				*app.client.borrow_mut() = Some(client.clone());
				let result = client.redirect_result().await;
				if app.generation.get() != generation {
					return Ok(());
				}
				app.state.update(|state| state.ready = true);
				if let Some(token) = result? {
					app.state.update(|state| state.busy = true);
					app.complete(token).await?;
				}
				Ok::<_, Error>(())
			}
			.await;
			if app.generation.get() == generation {
				if let Err(error) = result {
					app.failure(error).await;
				}
				app.state.update(|state| {
					state.loading = false;
					state.busy = false;
				});
			}
		});
	}
	async fn failure(&self, error: Error) {
		match error.code().as_deref() {
			Some("aidash/second-factor-required") => {
				self.password.set(String::new());
				self.state.update(|state| {
					state.factor = Some(Factor::Challenge);
					state.error = None;
				});
			}
			Some("aidash/enrollment-required") => {
				self.password.set(String::new());
				self.state.update(|state| {
					state.factor = Some(Factor::Enroll);
					state.error = None;
					state.enrollment = None;
				});
				let enrollment = async { self.client()?.start_enrollment().await }.await;
				match enrollment {
					Ok(value) => self.state.update(|state| state.enrollment = Some(value)),
					Err(error) => self.state.update(|state| state.error = Some(error.text())),
				}
			}
			_ => self.state.update(|state| state.error = Some(error.text())),
		}
	}
	async fn next(self: &Rc<Self>, path: &str, body: Value) -> Result<(), Error> {
		let context = self
			.state
			.get()
			.context
			.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
		let mut payload = json!({"flow":self.flow,"csrf":context.csrf});
		payload
			.as_object_mut()
			.expect("object")
			.extend(body.as_object().expect("action object").clone());
		let next: Next = api::request(path, Some(payload)).await?;
		if let Some(target) = next.redirect_to {
			api::navigate(&target)?;
		} else {
			self.reload();
		}
		Ok(())
	}
	async fn complete(self: &Rc<Self>, token: String) -> Result<(), Error> {
		// Close the upstream client before another proof is requested. The token
		// is a transient argument, never a Signal, URL or storage entry.
		self.clear_fields();
		self.state.update(|state| {
			state.factor = None;
			state.enrollment = None;
		});
		self.close().await?;
		self.next("/login/verify", json!({"id_token":token})).await
	}
	async fn perform(self: &Rc<Self>, action: Action) -> Result<(), Error> {
		match action {
			Action::Password => {
				let token = self
					.client()?
					.password(&self.email.get(), &self.password.get())
					.await?;
				self.complete(token).await?;
			}
			Action::Popup(provider) => {
				self.state.update(|state| state.redirect_provider = None);
				match self.client()?.popup(&provider).await {
					Ok(token) => self.complete(token).await?,
					Err(error) => {
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
				let sent = self
					.client()?
					.register(&self.email.get(), &self.password.get())
					.await?;
				self.password.set(String::new());
				self.state.update(|state|state.notice=Some(if sent { "Check your email and verify your address, then sign in to continue. This has not created an Aidash Account yet." } else { "Your login was created, but the verification email could not be sent. Enter your password and resend it." }.into()));
			}
			Action::Resend => {
				let sent = self
					.client()?
					.resend(&self.email.get(), &self.password.get())
					.await?;
				self.password.set(String::new());
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
					client.finish_enrollment(&self.code.get()).await?
				} else {
					client.resolve(&self.code.get()).await?
				};
				self.complete(token).await?;
			}
			Action::Tenant(tenant) => {
				self.clear_fields();
				self.close().await?;
				let context = self
					.state
					.get()
					.context
					.ok_or_else(|| Error::message("Start this sign-in step again."))?;
				let _: Value = api::request(
					"/login/tenant",
					Some(json!({"flow":self.flow,"csrf":context.csrf,"tenant_id":tenant})),
				)
				.await?;
				self.reload();
			}
			Action::Restart => {
				self.clear_fields();
				self.close().await?;
				self.reload();
			}
			Action::Choose(choice) => self.next("/login/choice", json!({"choice":choice})).await?,
			Action::AccountStart(path, identity) => {
				let account = self
					.state
					.get()
					.account
					.ok_or_else(|| Error::message("Start sign-in again from your product."))?;
				let mut body = json!({"csrf":account.csrf});
				if let Some(identity) = identity {
					body["identity_id"] = json!(identity);
				}
				let next: Next = api::request(path, Some(body)).await?;
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
				let result: Value =
					api::request("/logout/all", Some(json!({"csrf":account.csrf}))).await?;
				if result.get("completed").and_then(Value::as_bool) != Some(true) {
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
