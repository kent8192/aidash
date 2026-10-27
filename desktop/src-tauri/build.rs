fn main() {
	tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
		tauri_build::AppManifest::new().commands(&[
			"connection_settings",
			"save_connection",
			"select_connection",
			"remove_connection",
			"desktop_login",
			"desktop_access",
			"desktop_logout",
			"plan_listen",
			"plan_wait",
			"plan_cancel",
			"open_preview_url",
			"open_remote_preview",
		]),
	))
	.expect("build desktop application");
}
