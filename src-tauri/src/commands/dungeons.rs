use crate::engine;

#[tauri::command]
pub async fn dungeons_status() -> Result<engine::DungeonsStatus, String> {
    super::blocking(engine::dungeons_status).await
}

#[tauri::command]
pub async fn dungeons_ownership(account_id: String) -> Result<serde_json::Value, String> {
    engine::dungeons_ownership(&account_id).await
}

#[tauri::command]
pub async fn dungeons_install(app: tauri::AppHandle, account_id: String) -> Result<String, String> {
    engine::dungeons_install(app, account_id).await
}

#[tauri::command]
pub async fn dungeons_launch() -> Result<(), String> {
    super::blocking(engine::dungeons_launch).await?
}

/// The file picker is the only way in: the webview never names a source path.
#[tauri::command]
pub async fn dungeons_pick_mods() -> Result<usize, String> {
    let d = engine::dialog().set_title("Моды Dungeons (.pak)").add_filter("Unreal pak", &["pak"]);
    let Some(paths) = engine::pick_files(d).await else { return Ok(0) };
    super::blocking(move || engine::dungeons_add_mods(paths)).await?
}

#[tauri::command]
pub async fn dungeons_toggle_mod(name: String, enabled: bool) -> Result<(), String> {
    super::blocking(move || engine::dungeons_toggle_mod(&name, enabled)).await?
}

#[tauri::command]
pub async fn dungeons_remove_mod(name: String) -> Result<(), String> {
    super::blocking(move || engine::dungeons_remove_mod(&name)).await?
}

#[tauri::command]
pub fn dungeons_open_folder() {
    let dir = engine::dungeons_dir();
    let _ = std::fs::create_dir_all(&dir);
    engine::open_path(&dir.to_string_lossy());
}

#[tauri::command]
pub async fn store_games_state() -> Result<Vec<engine::StoreGameState>, String> {
    super::blocking(engine::store_games_state).await
}

#[tauri::command]
pub async fn store_game_open(slug: String, via: String) -> Result<(), String> {
    super::blocking(move || engine::store_game_open(&slug, &via)).await?
}

#[tauri::command]
pub async fn store_game_install(app: tauri::AppHandle, slug: String) -> Result<(), String> {
    engine::store_game_install(app, slug).await
}

#[tauri::command]
pub async fn game_ownership(account_id: String, slug: String) -> Result<serde_json::Value, String> {
    engine::game_ownership(&account_id, &slug).await
}

#[tauri::command]
pub async fn bedrock_join(host: String, port: u16) -> Result<(), String> {
    super::blocking(move || engine::bedrock_join(&host, port)).await?
}
