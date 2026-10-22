//! Tauri 命令层 / Tauri Command Layer
//!
//! 将 backend commands/ 模块中的业务函数包装为 `#[tauri::command]`，
//! 供前端通过 `invoke()` 调用。命令名与 server 模式下 HTTP 路由的语义一一对应。
//!
//! Wraps backend commands/ functions as `#[tauri::command]` for frontend
//! invocation via `invoke()`. Command names correspond to HTTP route semantics in server mode.

use crate::state::DesktopState;
use std::sync::Arc;
use tauri::State;

use stripchat_recorder_lib::{
    commands::{postprocess_cmd, recording_cmd, settings_cmd},
    config::settings::Settings,
    core::{emitter::EmitterExt, error::AppError},
    postprocess::pipeline::PipelineConfig,
};

type CmdResult<T> = std::result::Result<T, String>;

fn map_err(e: AppError) -> String {
    e.to_string()
}

// ─── Streamers ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_streamers(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let streamers = state.app_state.get_streamers();
    let has_any_status = streamers
        .iter()
        .any(|s| state.monitor.get_status(&s.username).is_some());

    // 若无任何缓存状态则触发一次轮询（非阻塞）
    // If no cached status exists, trigger a poll (non-blocking)
    if !has_any_status && !streamers.is_empty() {
        let monitor = Arc::clone(&state.monitor);
        let emitter = Arc::clone(&state.emitter);
        tokio::spawn(async move {
            monitor.poll_all_with_emitter(&emitter).await;
        });
    }

    let result: Vec<serde_json::Value> = streamers
        .into_iter()
        .map(|s| {
            let status = state.monitor.get_status(&s.username);
            serde_json::json!({
                "username": s.username,
                "auto_record": s.auto_record,
                "added_at": s.added_at,
                "is_online": status.as_ref().map(|st| st.is_online).unwrap_or(false),
                "is_recording": state.recorder.is_recording(&s.username),
                "is_recordable": status.as_ref().map(|st| st.is_recordable).unwrap_or(false),
                "viewers": status.as_ref().map(|st| st.viewers).unwrap_or(0),
                "status": status.as_ref().map(|st| st.status.clone()).unwrap_or_default(),
                "thumbnail_url": status.and_then(|st| st.thumbnail_url),
            })
        })
        .collect();
    Ok(serde_json::Value::Array(result))
}

#[tauri::command]
pub async fn add_streamer(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let username = username.trim().to_lowercase();
    if username.is_empty() {
        return Err("用户名不能为空".to_string());
    }
    let settings = state.app_state.get_settings();
    let api = stripchat_recorder_lib::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )
    .map_err(map_err)?;
    api.get_stream_info(&username, false).await.map_err(map_err)?;
    state.app_state.add_streamer(&username).map_err(map_err)?;
    state.emitter.emit("streamer-added", &serde_json::json!({ "username": username }));
    let emitter = Arc::clone(&state.emitter);
    let monitor = Arc::clone(&state.monitor);
    tokio::spawn(async move {
        monitor.poll_one_with_emitter(&username, &emitter).await;
    });
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn remove_streamer(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    if state.recorder.is_recording(&username) {
        state.recorder.stop_recording(&username).await.map_err(map_err)?;
    }
    let settings = state.app_state.get_settings();
    // 分片目录与最终视频目录都可能有该主播的文件夹，需一并删除
    // Both the segment dir and the final video dir may hold a folder for this streamer
    for root in settings.video_scan_roots() {
        let dir = root.join(&username);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    state.app_state.remove_streamer(&username).map_err(map_err)?;
    state.emitter.emit("streamer-removed", &serde_json::json!({ "username": username }));
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn set_auto_record(
    username: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.set_auto_record(&username, enabled).map_err(map_err)?;
    state.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": username, "enabled": enabled }),
    );
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn start_recording(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let playlist_url = if let Some(url) = state.monitor.get_cached_playlist_url(&username) {
        url
    } else {
        let settings = state.app_state.get_settings();
        let api = stripchat_recorder_lib::streaming::stripchat::StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
        )
        .map_err(map_err)?
        .with_mouflon_keys(state.app_state.get_mouflon_keys())
        .with_resolution_selection(
            settings.preferred_resolution,
            &settings.resolution_preference,
        );
        let info = api.get_stream_info(&username, true).await.map_err(map_err)?;
        info.playlist_url
            .ok_or_else(|| format!("Stream offline: {}", username))?
    };
    let path = state
        .recorder
        .start_recording_with_emitter(&username, &playlist_url, Arc::clone(&state.emitter))
        .await
        .map_err(map_err)?;
    Ok(serde_json::json!({ "path": path }))
}

#[tauri::command]
pub async fn stop_recording(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let _ = state.app_state.set_auto_record(&username, false);
    state.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": username, "enabled": false }),
    );
    state.recorder.stop_recording(&username).await.map_err(map_err)?;
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn verify_streamer(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let api = stripchat_recorder_lib::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )
    .map_err(map_err)?;
    match api.get_stream_info(&username, false).await {
        Ok(_) => Ok(serde_json::json!({ "exists": true })),
        Err(AppError::UserNotFound(_)) => Ok(serde_json::json!({ "exists": false })),
        Err(e) => Err(e.to_string()),
    }
}

// ─── Settings ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_settings(state: State<'_, DesktopState>) -> CmdResult<Settings> {
    Ok(state.app_state.get_settings())
}

#[tauri::command]
pub async fn save_settings_cmd(
    new_settings: Settings,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.update_settings(new_settings).map_err(map_err)?;
    state.emitter.emit("settings-updated", &state.app_state.get_settings());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn get_disk_space(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let output_dir = state.app_state.get_settings().output_dir;
    let result = tokio::task::spawn_blocking(move || {
        settings_cmd::get_disk_space_inner(&output_dir)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    Ok(serde_json::to_value(result).unwrap())
}

// ─── Mouflon Keys ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_mouflon_keys(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    Ok(serde_json::to_value(state.app_state.get_mouflon_keys_store()).unwrap())
}

#[tauri::command]
pub async fn add_mouflon_key(
    pkey: String,
    pdkey: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.add_mouflon_key(&pkey, &pdkey).map_err(map_err)?;
    state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn remove_mouflon_key(
    pkey: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.remove_mouflon_key(&pkey).map_err(map_err)?;
    state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn sync_mouflon_keys(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let url = settings
        .mouflon_sync_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or("未配置 mouflon_sync_url")?
        .to_string();
    let token = settings.mouflon_sync_token.clone();
    let updated = state
        .app_state
        .sync_mouflon_keys_from_worker(&url, token.as_deref())
        .await
        .map_err(map_err)?;
    if updated {
        state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    }
    Ok(serde_json::json!({ "updated": updated }))
}

// ─── Recordings ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_recordings(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let app_state = Arc::clone(&state.app_state);
    let recorder = Arc::clone(&state.recorder);
    let files = tokio::task::spawn_blocking(move || {
        recording_cmd::list_recordings_inner(&app_state, &recorder)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(files).unwrap())
}

#[tauri::command]
pub async fn get_merging_dirs(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let merge_format = settings.merge_format.clone();

    let make_entry = |path: &std::path::PathBuf, status: &str| {
        let path_str = path.to_string_lossy().to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        // 合并目标位于最终视频目录（可能与分片目录不同）
        // The merge target lives in the final video directory (may differ from the segment dir)
        let merged_path = settings
            .video_path_for_session_with_format(path, &merge_format)
            .to_string_lossy()
            .to_string();
        serde_json::json!({
            "session_dir": path_str,
            "merged_path": merged_path,
            "merge_format": merge_format,
            "username": username,
            "status": status,
        })
    };

    let mut result: Vec<serde_json::Value> = state
        .recorder
        .merging_dirs
        .read()
        .iter()
        .map(|p| make_entry(p, "merging"))
        .collect();
    result.extend(
        state.recorder.waiting_merge_dirs.read().iter().map(|p| make_entry(p, "waiting")),
    );
    Ok(serde_json::json!(result))
}

#[tauri::command]
pub async fn delete_recording(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let recorder = Arc::clone(&state.recorder);
    let app_state = Arc::clone(&state.app_state);
    let path_clone = path.clone();
    tokio::task::spawn_blocking(move || {
        recording_cmd::delete_recording_inner(&path_clone, &recorder, &app_state)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    state.emitter.emit("recording-deleted", &serde_json::json!({ "path": path }));
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Post-processing ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn run_postprocess_cmd(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let pipeline = state.app_state.get_pipeline();
    if pipeline.nodes.is_empty() {
        return Err("后处理流水线为空".to_string());
    }
    let video_path = std::path::PathBuf::from(&path);
    let emitter = Arc::clone(&state.emitter);
    let app_state = Arc::clone(&state.app_state);
    app_state.pp_task_enqueue(&path);
    emitter.emit("postprocess-waiting", &serde_json::json!({ "path": path }));
    tokio::task::spawn_blocking(move || {
        postprocess_cmd::run_postprocess_for_path_inner(&video_path, &pipeline, &emitter, &app_state);
    });
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn run_postprocess_batch(
    paths: Vec<String>,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let pipeline = state.app_state.get_pipeline();
    if pipeline.nodes.is_empty() {
        return Err("后处理流水线为空".to_string());
    }
    for path in paths {
        let video_path = std::path::PathBuf::from(&path);
        let emitter = Arc::clone(&state.emitter);
        let app_state = Arc::clone(&state.app_state);
        let pipeline = pipeline.clone();
        app_state.pp_task_enqueue(&path);
        emitter.emit("postprocess-waiting", &serde_json::json!({ "path": path }));
        tokio::task::spawn_blocking(move || {
            postprocess_cmd::run_postprocess_for_path_inner(&video_path, &pipeline, &emitter, &app_state);
        });
    }
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn cancel_postprocess(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.pp_task_cancel(&path);
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn get_postprocess_tasks(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    Ok(serde_json::to_value(state.app_state.get_pp_tasks()).unwrap())
}

// ─── Pipeline ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_pipeline(state: State<'_, DesktopState>) -> CmdResult<PipelineConfig> {
    Ok(state.app_state.get_pipeline())
}

#[tauri::command]
pub async fn save_pipeline(
    pipeline: PipelineConfig,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.update_pipeline(pipeline).map_err(map_err)?;
    state.emitter.emit("pipeline-updated", &state.app_state.get_pipeline());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn list_modules() -> CmdResult<serde_json::Value> {
    let modules = tokio::task::spawn_blocking(
        stripchat_recorder_lib::postprocess::pipeline::discover_modules,
    )
    .await
    .unwrap_or_default();
    Ok(serde_json::to_value(modules).unwrap())
}

/// 以自检模式（`PP_TEST=1`）运行指定模块并返回完整输出日志。
/// 用于在界面上验证模块配置（例如 Telegram 的机器人凭据、群组与话题）。
///
/// Run the given module in self-test mode (`PP_TEST=1`) and return its full output log.
/// Used to verify a module's configuration from the UI (e.g. Telegram bot credentials,
/// chat and forum topic).
#[tauri::command]
pub async fn test_module(
    module_id: String,
    params: std::collections::HashMap<String, serde_json::Value>,
) -> CmdResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(move || {
        let module = stripchat_recorder_lib::postprocess::pipeline::discover_modules()
            .into_iter()
            .find(|m| m.id == module_id);
        match module {
            Some(m) => stripchat_recorder_lib::postprocess::pipeline::run_module_test(&m, &params),
            None => stripchat_recorder_lib::postprocess::pipeline::TestResult {
                success: false,
                message: format!("模块 '{}' 不存在", module_id),
                logs: vec![],
            },
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(result).unwrap())
}

/// 启动模块的交互式登录，返回第一状态（`ready` / `already` / `error`）。
///
/// Start the module's interactive login and return the first state
/// (`ready` / `already` / `error`).
#[tauri::command]
pub async fn login_start(
    module_id: String,
    params: std::collections::HashMap<String, serde_json::Value>,
) -> CmdResult<serde_json::Value> {
    let update = tokio::task::spawn_blocking(move || {
        let module = stripchat_recorder_lib::postprocess::pipeline::discover_modules()
            .into_iter()
            .find(|m| m.id == module_id);
        match module {
            Some(m) => stripchat_recorder_lib::postprocess::login::start_login(&m, &params),
            None => Err(format!("模块 '{}' 不存在", module_id)),
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(serde_json::to_value(update).unwrap())
}

/// 向登录会话提交一行命令（手机号 / 验证码 / 密码），返回下一个状态。
///
/// Submit one command line (phone / code / password) to a login session and return
/// the next state.
#[tauri::command]
pub async fn login_input(login_id: String, value: String) -> CmdResult<serde_json::Value> {
    let update = tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::postprocess::login::send_login_input(&login_id, &value)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(serde_json::to_value(update).unwrap())
}

/// 取消并结束一个登录会话 / Cancel and terminate a login session.
#[tauri::command]
pub async fn login_cancel(login_id: String) -> CmdResult<serde_json::Value> {
    tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::postprocess::login::cancel_login(&login_id)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Locale ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_locale(locale_code: String) -> CmdResult<serde_json::Value> {
    let lc = locale_code.clone();
    let (locale, warning) = tokio::task::spawn_blocking(move || {
        let data = stripchat_recorder_lib::locale::manager::get_full_locale(&lc);
        let warning = stripchat_recorder_lib::locale::manager::validate_locale_file(&lc);
        (data, warning)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut result = locale;
    if let Some(w) = warning {
        result["warning"] = serde_json::Value::String(w);
    }
    Ok(result)
}

#[tauri::command]
pub async fn list_locales() -> CmdResult<serde_json::Value> {
    let locales = tokio::task::spawn_blocking(
        stripchat_recorder_lib::locale::manager::list_available_locales,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(locales).unwrap())
}

// ─── Startup Warnings ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_startup_warnings(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let app_state = Arc::clone(&state.app_state);
    let result = tokio::task::spawn_blocking(move || {
        let data = app_state.data.read();
        let missing_pp_results: Vec<String> = data
            .pp_results
            .iter()
            .filter(|p| !std::path::Path::new(p.as_str()).exists())
            .cloned()
            .collect();
        serde_json::json!({
            "missing_streamers": [],
            "missing_pp_results": missing_pp_results,
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn remove_missing_pp_results(
    paths: Vec<String>,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let mut data = state.app_state.data.write();
    data.pp_results.retain(|p| !paths.contains(p));
    drop(data);
    state.app_state.save().map_err(map_err)?;
    Ok(serde_json::json!({ "ok": true }))
}

// ─── File Ops ─────────────────────────────────────────────────────────────────

/// 打开录制文件（用系统默认播放器）/ Open a recording file with the system default player
#[tauri::command]
pub async fn open_recording(
    path: String,
    app: tauri::AppHandle,
) -> CmdResult<serde_json::Value> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(&path, None::<&str>).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 在文件管理器中打开输出目录 / Open the output directory in the file manager
#[tauri::command]
pub async fn open_output_dir(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    use tauri_plugin_opener::OpenerExt;
    // 最终视频目录（未单独设置时即为输出目录）/ The final video directory (equals output dir when unset)
    let video_dir = state.app_state.get_settings().videos_root();
    app.opener().open_path(&video_dir, None::<&str>).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 读取输出目录中的图片文件（base64 编码，用于缩略图显示）
/// Read an image file in the output directory as base64 (for thumbnail display)
#[tauri::command]
pub async fn read_output_file(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let requested = std::path::Path::new(&path);

    // 安全检查：确保请求的文件在最终视频目录或输出目录范围内
    // Safety check: ensure the requested file is within the video dir or the output dir
    let allowed_roots: Vec<std::path::PathBuf> = settings
        .video_scan_roots()
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    let canonical_requested = requested.canonicalize().map_err(|_| "文件不存在".to_string())?;
    if !allowed_roots.iter().any(|root| canonical_requested.starts_with(root)) {
        return Err("访问被拒绝".to_string());
    }

    let data = std::fs::read(&canonical_requested).map_err(|e| e.to_string())?;
    let ext = canonical_requested
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mime = match ext {
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => "application/octet-stream",
    };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(serde_json::json!({
        "data": format!("data:{};base64,{}", mime, b64),
        "mime": mime,
    }))
}

/// 获取模块输出路径 / Get module output paths
#[tauri::command]
pub async fn get_module_outputs(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let video_path = std::path::Path::new(&path);
    let pipeline = state.app_state.get_pipeline();
    let mut outputs: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for node in &pipeline.nodes {
        if !node.enabled {
            continue;
        }
        if node.module_id == "contact_sheet" {
            let format = node
                .params
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("webp");
            if let (Some(parent), Some(stem)) = (
                video_path.parent(),
                video_path.file_stem().and_then(|s| s.to_str()),
            ) {
                let img_path = parent.join(format!("{}.{}", stem, format));
                if img_path.exists() {
                    outputs.insert(node.module_id.clone(), img_path.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(serde_json::to_value(outputs).unwrap())
}
