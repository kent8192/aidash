//! Typed JS interop only for the upstream Firebase SDK. No UI is delegated.
use crate::{
	api::Error,
	model::{Enrollment, LoginContext},
};
use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
	#[wasm_bindgen(catch, js_namespace = globalThis, js_name = aidashCreateGcipClient)]
	fn create(config: &JsValue) -> Result<Promise, JsValue>;
	#[derive(Clone)]
	pub type Client;
	#[wasm_bindgen(method, structural, catch, js_name = password)]
	fn password_js(this: &Client, email: &str, password: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = popup)]
	fn popup_js(this: &Client, provider: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = redirect)]
	fn redirect_js(this: &Client, provider: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = redirectResult)]
	fn redirect_result_js(this: &Client) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = register)]
	fn register_js(this: &Client, email: &str, password: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = resend)]
	fn resend_js(this: &Client, email: &str, password: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = resolveSecondFactor)]
	fn resolve_js(this: &Client, code: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = startEnrollment)]
	fn start_enrollment_js(this: &Client) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = finishEnrollment)]
	fn finish_enrollment_js(this: &Client, code: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, structural, catch, js_name = close)]
	fn close_js(this: &Client) -> Result<Promise, JsValue>;
}
async fn token(promise: Result<Promise, JsValue>) -> Result<String, Error> {
	JsFuture::from(promise?).await?.as_string().ok_or_else(|| {
		Error::message("The identity provider did not return a proof. Start this step again.")
	})
}
impl Client {
	pub async fn new(context: &LoginContext) -> Result<Self, Error> {
		let config = serde_json::json!({"project_id":context.firebase.project_id,"api_key":context.firebase.api_key,
			"auth_domain":context.firebase.auth_domain,"tenant_id":context.selected_tenant_id,"providers":context.providers,
			"password_sign_up":context.password_sign_up,"mfa_mandatory":context.mfa_mandatory});
		let config = js_sys::JSON::parse(&config.to_string())?;
		Ok(JsFuture::from(create(&config)?).await?.unchecked_into())
	}
	pub async fn password(&self, email: &str, password: &str) -> Result<String, Error> {
		token(self.password_js(email, password)).await
	}
	pub async fn popup(&self, provider: &str) -> Result<String, Error> {
		token(self.popup_js(provider)).await
	}
	pub async fn redirect(&self, provider: &str) -> Result<(), Error> {
		JsFuture::from(self.redirect_js(provider)?).await?;
		Ok(())
	}
	pub async fn redirect_result(&self) -> Result<Option<String>, Error> {
		let value = JsFuture::from(self.redirect_result_js()?).await?;
		if value.is_null() || value.is_undefined() {
			Ok(None)
		} else {
			value
				.as_string()
				.map(Some)
				.ok_or_else(|| Error::message("Invalid identity provider response"))
		}
	}
	pub async fn register(&self, email: &str, password: &str) -> Result<bool, Error> {
		Ok(JsFuture::from(self.register_js(email, password)?)
			.await?
			.as_bool()
			.unwrap_or(false))
	}
	pub async fn resend(&self, email: &str, password: &str) -> Result<bool, Error> {
		Ok(JsFuture::from(self.resend_js(email, password)?)
			.await?
			.as_bool()
			.unwrap_or(false))
	}
	pub async fn resolve(&self, code: &str) -> Result<String, Error> {
		token(self.resolve_js(code)).await
	}
	pub async fn start_enrollment(&self) -> Result<Enrollment, Error> {
		serde_wasm_bindgen::from_value(JsFuture::from(self.start_enrollment_js()?).await?)
			.map_err(|_| Error::message("Invalid enrollment response"))
	}
	pub async fn finish_enrollment(&self, code: &str) -> Result<String, Error> {
		token(self.finish_enrollment_js(code)).await
	}
	pub async fn close(&self) -> Result<(), Error> {
		JsFuture::from(self.close_js()?).await?;
		Ok(())
	}
}
