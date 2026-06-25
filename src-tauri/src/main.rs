#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod diskusage;
mod index;
mod ipc;
mod jobs;
mod portable;
mod rename;
mod roots;
mod search;
mod settings;
mod state;
mod walker;

use std::sync::atomic::Ordering;

fn main() {
    let app_state = state::new_app_state();
    let close_to_tray_flag = app_state.close_to_tray.clone();
    let tray_available_flag = app_state.tray_available.clone();

    tauri::Builder::default()
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            use tauri::Manager;

            // Set the runtime window icon so the X11/Wayland window manager,
            // taskbar, and dock all show the Cove skull instead of a generic
            // placeholder. Bundle icons cover AppImage/deb/installer; this
            // covers the live window for both `tauri dev` and packaged builds.
            let icon_bytes: &[u8] = include_bytes!("../icons/icon.png");
            let tray_image = tauri::image::Image::from_bytes(icon_bytes).ok();
            if let Some(image) = tray_image.clone() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_icon(image);
                }
            }

            // Build tray menu: Show / Hide / Quit.
            let show_item = MenuItem::with_id(app, "tray_show", "Show", true, None::<&str>)?;
            let hide_item = MenuItem::with_id(app, "tray_hide", "Hide", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "tray_quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &hide_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::with_id("cove-tray")
                .tooltip("Cove Toolkit")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "tray_show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "tray_hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "tray_quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left-click toggles main window visibility.
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let visible = w.is_visible().unwrap_or(false);
                            if visible {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                        }
                    }
                });
            if let Some(image) = tray_image {
                tray_builder = tray_builder.icon(image);
            }
            // Tray creation can fail on Linux if no system tray host is
            // running (e.g. GNOME without AppIndicator). On success the
            // returned TrayIcon must be retained for the lifetime of the
            // app — dropping it removes the icon — so we hand it to
            // app.manage(). On failure we leave tray_available=false so
            // the close handler will not hide the window with no way back.
            match tray_builder.build(app) {
                Ok(tray) => {
                    app.manage(tray);
                    app.state::<state::AppState>()
                        .tray_available
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("[tray] failed to create tray icon: {e}. Close-to-tray is disabled this session to keep the window reachable.");
                }
            }
            Ok(())
        })
        .on_window_event({
            let close_to_tray_flag = close_to_tray_flag.clone();
            let tray_available_flag = tray_available_flag.clone();
            move |window, event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if window.label() == "main"
                        && close_to_tray_flag.load(Ordering::Relaxed)
                        && tray_available_flag.load(Ordering::Relaxed)
                    {
                        api.prevent_close();
                        let _ = window.hide();
                        // Notify the frontend so it can show a one-time
                        // "still running in tray" toast on first hide.
                        use tauri::Emitter;
                        let _ = window.emit("cove://close-to-tray", ());
                    }
                }
            }
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
            ipc::rescan_disk_dir,
            ipc::move_to_trash,
            ipc::delete_permanently,
            ipc::rename_path,
            ipc::copy_paths,
            ipc::move_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
