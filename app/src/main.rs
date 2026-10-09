//! Runout desktop app: Tauri shell around the engine.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod commands;
mod lookup;
mod player;
mod prefs;
mod ring;
mod state;

use state::App;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{Emitter, Manager};

fn main() {
    // WebKitGTK's DMA-BUF renderer gives blank or flickering windows on some AMD
    // and Wayland setups. Turning it off is the known workaround.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App::new())
        .setup(|app| {
            let handle = app.handle().clone();
            // Transport, meters and capture levels, 20 times a second.
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(50));
                let st = handle.state::<App>();
                let sh = &st.player.shared;
                let (pl, pr) = sh.take_peaks();
                let db = |v: f32| if v <= 1e-6 { -120.0 } else { 20.0 * v.log10() };
                let meters = *sh.meters.lock().unwrap();
                let cap = commands::capture_status_of(&st);
                let payload = serde_json::json!({
                    "playing": sh.playing.load(Ordering::Relaxed),
                    "pos": sh.pos(),
                    "warn": sh.warn.lock().unwrap().clone(),
                    "meters": meters,
                    "peak": [db(pl), db(pr)],
                    "capture": cap,
                    "busy": st.busy.lock().unwrap().clone(),
                });
                let _ = handle.emit("tick", payload);
            });
            // Autosave every 30 seconds.
            let h2 = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                h2.state::<App>().autosave();
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::recovery_dismiss,
            commands::project_new,
            commands::project_open,
            commands::project_get,
            commands::project_save,
            commands::project_set,
            commands::undo,
            commands::redo,
            commands::import_files,
            commands::folder_plan,
            commands::folder_commit,
            commands::remove_source,
            commands::split_auto,
            commands::split_auto_threshold,
            commands::mark_boundary,
            commands::add_track,
            commands::merge_next,
            commands::delete_track,
            commands::set_cleaned,
            commands::set_override,
            commands::auto_stage,
            commands::learn_noise,
            commands::add_repair,
            commands::remove_repair,
            commands::analyze,
            commands::cancel_job,
            commands::clicks,
            commands::presets_list,
            commands::preset_apply,
            commands::preset_save,
            commands::preset_delete,
            commands::export_run,
            commands::open_folder,
            commands::read_report,
            commands::lookup_search,
            commands::lookup_release,
            commands::apply_tracklist,
            commands::devices,
            commands::select_device,
            commands::level_check,
            commands::capture_start,
            commands::capture_stop,
            commands::recover_capture,
            commands::play,
            commands::pause,
            commands::seek,
            commands::set_loop,
            commands::set_monitor,
            commands::batch_plan,
            commands::batch_run,
            commands::tile_wave,
            commands::tile_spec,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Runout")
        .run(|handle, event| {
            if let tauri::RunEvent::Exit = event {
                let st = handle.state::<App>();
                st.autosave();
                if st.capture.lock().unwrap().is_none() {
                    commands::mark_closed(&st);
                }
            }
        });
}
