#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod diskusage;
mod index;
mod ipc;
mod jobs;
mod rename;
mod roots;
mod search;
mod settings;
mod state;
mod walker;

fn main() {
    let app_state = state::new_app_state();

    tauri::Builder::default()
        .setup(|app| {
            use tauri::Manager;
            // Set the runtime window icon so the X11/Wayland window manager,
            // taskbar, and dock all show the Cove skull instead of a generic
            // placeholder. Bundle icons cover AppImage/deb/installer; this
            // covers the live window for both `tauri dev` and packaged builds.
            let icon_bytes: &[u8] = include_bytes!("../icons/icon.png");
            if let Ok(image) = tauri::image::Image::from_bytes(icon_bytes) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_icon(image);
                }
            }
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            ipc::scan_index,
            ipc::scan_index_multi,
            ipc::start_index_all,
            ipc::rescan_index_root,
            ipc::search,
            ipc::cancel_job,
            ipc::get_index_stats,
            ipc::get_index_scan_state,
            ipc::get_index_roots,
            ipc::add_index_root,
            ipc::remove_index_root,
            ipc::update_index_root_enabled,
            ipc::detect_index_roots,
            ipc::scan_disk_usage,
            ipc::cancel_disk_usage_scan,
            ipc::get_disk_usage_scan_state,
            ipc::get_disk_usage,
            ipc::preview_rename,
            ipc::apply_rename,
            ipc::undo_rename,
            ipc::get_settings,
            ipc::save_settings,
            ipc::get_cache_info,
            ipc::load_cached_index,
            ipc::clear_cache,
            ipc::open_path,
            ipc::reveal_in_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
