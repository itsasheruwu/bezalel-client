#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_models;
mod builder;
mod builder_setup;
mod commands;
mod draft_files;
mod minecraft;
mod mod_installation;
mod packs;
mod secrets;
mod sharing_auth;
mod state;
mod studio;
mod theme;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            let state = state::State::new(root).map_err(std::io::Error::other)?;
            let _ = theme::apply(
                app.handle(),
                &state.data().map_err(std::io::Error::other)?.settings.theme,
            );
            app.manage(state);
            studio::set_app(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::save_settings,
            commands::complete_onboarding,
            commands::create_draft,
            commands::delete_draft,
            packs::remove_local_mod,
            commands::save_draft_idea,
            commands::studio_chat,
            commands::agent_models,
            commands::save_agent_preferences,
            commands::apply_app_theme,
            commands::attach_studio_file,
            commands::studio_cancel,
            commands::generate_mod_source,
            commands::build_mod,
            commands::prepare_mod_tools,
            commands::open_minecraft_eula,
            commands::export_jar,
            commands::connect_chatgpt,
            commands::disconnect_chatgpt,
            commands::connect_microsoft,
            commands::disconnect_microsoft,
            commands::play,
            commands::open_instance,
            commands::prepare_handoff,
            commands::open_official_loader,
            commands::open_minecraft_launcher,
            commands::export_diagnostics,
            packs::create_pack,
            packs::delete_pack,
            commands::save_mod_source,
            commands::mod_sources,
            commands::open_mod_workspace,
            commands::studio_respond,
            packs::set_local_server_address,
            packs::sync_connect,
            packs::sync_sign_in,
            packs::create_sync_access_code,
            packs::save_sync_profile,
            packs::sync_disconnect,
            packs::refresh_packs,
            packs::sync_pack,
            packs::set_pack_trust,
            packs::invite_preview,
            packs::join_pack,
            packs::pack_details,
            packs::invite_create,
            packs::invite_revoke,
            packs::member_remove,
            packs::submit_mod,
            packs::import_local_mod,
            packs::install_local_mod,
            mod_installation::move_installed_mod,
            mod_installation::delete_installed_mod,
            packs::create_candidate,
            packs::export_candidate,
            packs::publish_candidate,
        ])
        .run(tauri::generate_context!())
        .expect("Bezalel could not start");
}
