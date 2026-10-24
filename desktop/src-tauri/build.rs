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
		]),
	))
	.expect("build desktop application");
}
