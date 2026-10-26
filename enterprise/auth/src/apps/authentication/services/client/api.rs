// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! GCIP error presentation and final redirect validation; data uses server_fn.
use wasm_bindgen::JsValue;

pub enum Error {
	Message(String),
	Upstream(JsValue),
}
impl From<JsValue> for Error {
	fn from(value: JsValue) -> Self {
		Self::Upstream(value)
	}
}
impl Error {
	pub fn message(value: impl Into<String>) -> Self {
		Self::Message(value.into())
	}
	pub fn code(&self) -> Option<String> {
		match self {
			Self::Upstream(value) => js_sys::Reflect::get(value, &"code".into())
				.ok()?
				.as_string(),
			_ => None,
		}
	}
	pub fn text(&self) -> String {
		if let Self::Message(value) = self {
			return value.clone();
		}
		match self.code().as_deref() {
			Some("auth/invalid-verification-code") => "That verification code is not correct. Check your authenticator and try again.",
			Some("auth/code-expired" | "auth/invalid-multi-factor-session" | "auth/missing-multi-factor-session") => "This verification has expired. Start this sign-in step again.",
			Some("auth/too-many-requests") => "Too many attempts. Wait a few minutes before trying again.",
			Some("auth/popup-blocked") => "Your browser blocked the sign-in window. Allow popups or continue in this tab.",
			Some("auth/popup-closed-by-user") => "The sign-in window was closed. Try again when you are ready.",
			Some("auth/weak-password") => "Choose a stronger password and try again.",
			Some("aidash/second-factor-unsupported") => "This sign-in method requires a second factor this page cannot complete. Use your identity provider's recovery or contact your administrator.",
			_ => "Sign-in could not be completed. Check your details and try again, or use your identity provider's recovery.",
		}.into()
	}
}

impl From<reinhardt::pages::server_fn::ServerFnError> for Error {
	fn from(error: reinhardt::pages::server_fn::ServerFnError) -> Self {
		Self::Message(error.user_message().to_owned())
	}
}

pub fn navigate(target: &str) -> Result<(), Error> {
	let invalid = || Error::message("The sign-in response contains an unsupported redirect.");
	let location = web_sys::window().ok_or_else(invalid)?.location();
	let origin = location.origin()?;
	let url = web_sys::Url::new_with_base(target, &origin).map_err(|_| invalid())?;
	let loopback = |host: &str| matches!(host, "localhost" | "127.0.0.1" | "[::1]");
	let development = location.protocol()? == "http:"
		&& loopback(&location.hostname()?)
		&& loopback(&url.hostname());
	if !url.username().is_empty()
		|| !url.password().is_empty()
		|| !matches!(url.protocol().as_str(), "http:" | "https:")
		|| (url.protocol() != "https:" && url.origin() != origin && !development)
	{
		return Err(invalid());
	}
	location.assign(&url.href())?;
	Ok(())
}
