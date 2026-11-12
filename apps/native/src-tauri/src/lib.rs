//! Native shell is deliberately thin; domain, indexers, storage, and HTTP
//! execution remain in the shared IndexLoom Rust workspace.
mod commands;
mod secrets;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::NativeState::default())
        .invoke_handler(tauri::generate_handler![
            commands::start_autonomous,
            commands::validate_external,
            commands::platform_info,
        ])
        .setup(|app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("IndexLoom")
            .inner_size(1080.0, 760.0)
            .on_navigation(|url| {
                // Never allow file/data/javascript URLs or an HTTP remote host.
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("127.0.0.1" | "tauri.localhost")))
                    || (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
            })
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("native IndexLoom shell failed");
}
