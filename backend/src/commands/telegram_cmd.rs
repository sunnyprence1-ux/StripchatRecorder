//! 手动发送到 Telegram / Manual "send to Telegram" Command
//!
//! 允许用户从录制列表中挑选任意成片，立即发送到 Telegram 群组，而不必等待整条后处理流水线。
//! 复用 `notify_telegram` 模块本身：把流水线节点里保存的参数作为默认值，
//! 再由请求里的覆盖项（群组 ID、话题开关、发送后删除等）逐项替换。
//!
//! Lets the user pick any finished video from the recordings list and send it to a Telegram
//! group right away, without waiting for the whole post-processing pipeline. It reuses the
//! `notify_telegram` module itself: the parameters saved on the pipeline node act as defaults,
//! and the request's overrides (chat id, topic toggle, delete-after-send, …) replace them
//! one by one.

use crate::config::settings::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::postprocess::pipeline::{discover_modules, run_module_once, ModuleInfo};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Telegram 通知模块的固定 ID / Fixed module id of the Telegram notification module
pub const TELEGRAM_MODULE_ID: &str = "notify_telegram";

/// 手动发送请求（全部覆盖项都是可选的，缺省时沿用流水线节点里的配置）。
/// Manual-send request (every override is optional; the pipeline node's values are the defaults).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    /// 要发送的成片路径 / Path of the video to send
    pub path: String,
    /// 覆盖的群组 ID（空 = 用流水线里的值）/ Chat id override (empty = the pipeline value)
    #[serde(default)]
    pub chat_id: Option<String>,
    /// 覆盖的群组 username / Username override
    #[serde(default)]
    pub username: Option<String>,
    /// 覆盖「按模特分话题」/ `topic_per_model` override
    #[serde(default)]
    pub topic_per_model: Option<bool>,
    /// 覆盖「发送后删除本地数据」/ `delete_after_send` override
    #[serde(default)]
    pub delete_after_send: Option<bool>,
    /// 覆盖「同时发送视频文件」/ `send_video` override
    #[serde(default)]
    pub send_video: Option<bool>,
    /// 覆盖「附带文字说明」/ `send_caption` override
    #[serde(default)]
    pub send_caption: Option<bool>,
    /// 覆盖「同时发送封面图」/ `send_cover` override
    #[serde(default)]
    pub send_cover: Option<bool>,
    /// 覆盖「先单独发封面图」/ `cover_first` override
    #[serde(default)]
    pub cover_first: Option<bool>,
    /// 同时上传多个文件时的批量大小（1=逐个，>1=并发）/ batch size for concurrent uploads
    #[serde(default)]
    pub batch_size: Option<u32>,
}

/// 手动发送界面需要的默认值（来自流水线节点，供前端预填表单）。
/// Defaults for the manual-send UI (taken from the pipeline node, used to pre-fill the form).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendDefaults {
    /// 是否找到 Telegram 模块 / Whether the Telegram module was found
    pub available: bool,
    /// 模块名称（含版本）/ Module name (with version)
    pub module_name: String,
    /// 默认群组 ID / Default chat id
    pub chat_id: String,
    /// 默认群组 username / Default username
    pub username: String,
    /// 默认「按模特分话题」/ Default `topic_per_model`
    pub topic_per_model: bool,
    /// 默认「发送后删除本地数据」/ Default `delete_after_send`
    pub delete_after_send: bool,
    /// 默认「同时发送视频文件」/ Default `send_video`
    pub send_video: bool,
    /// 默认「附带文字说明」/ Default `send_caption`
    pub send_caption: bool,
    /// 默认「同时发送封面图」/ Default `send_cover`
    pub send_cover: bool,
    /// 默认「先单独发封面图，再发视频」/ Default `cover_first`
    pub cover_first: bool,
    /// 默认批量上传大小 / Default batch size
    pub batch_size: u32,
    /// 登录方式（auto / user / bot）/ Login mode
    pub account: String,
    /// 是否已有可用账号（已登录用户会话或机器人 Token）
    pub has_account: bool,
    /// 流水线中是否已配置该模块节点 / Whether the pipeline already has a node for this module
    pub in_pipeline: bool,
}

/// 在模块列表中查找 Telegram 模块 / Find the Telegram module in the module list
pub fn find_module() -> Option<ModuleInfo> {
    discover_modules()
        .into_iter()
        .find(|m| m.id == TELEGRAM_MODULE_ID)
}

/// 读取流水线中 Telegram 节点的参数作为默认值。
/// Read the Telegram node's parameters from the pipeline as defaults.
pub(crate) fn pipeline_params(state: &AppState) -> HashMap<String, serde_json::Value> {
    let pipeline = state.get_pipeline();
    pipeline
        .nodes
        .iter()
        .find(|n| n.module_id == TELEGRAM_MODULE_ID)
        .map(|n| n.params.clone())
        .unwrap_or_default()
}

/// 取参数中的字符串值 / Read a string parameter value
pub(crate) fn param_str(params: &HashMap<String, serde_json::Value>, key: &str) -> String {
    match params.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 取参数中的布尔值（缺省用 fallback）/ Read a boolean parameter (fallback when absent)
fn param_bool(params: &HashMap<String, serde_json::Value>, key: &str, fallback: bool) -> bool {
    match params.get(key) {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => fallback,
    }
}

/// 收集手动发送界面需要的默认值。
/// Collect the defaults the manual-send UI needs.
pub fn send_defaults(state: &AppState) -> SendDefaults {
    let module = find_module();
    let pipeline = state.get_pipeline();
    let in_pipeline = pipeline
        .nodes
        .iter()
        .any(|n| n.module_id == TELEGRAM_MODULE_ID);
    let params = pipeline_params(state);

    let account = {
        let a = param_str(&params, "account");
        if a.is_empty() { "auto".to_string() } else { a }
    };
    let api_id = param_str(&params, "api_id");
    let bot_token = param_str(&params, "bot_token");
    // 与模块自身的判定保持一致：auto 时有已登录的用户会话就算有账号
    // Match the module's own rule: with `auto`, a signed-in user session counts as an account
    let has_user_session = api_id
        .trim()
        .parse::<i32>()
        .map(user_session_exists)
        .unwrap_or(false);
    let has_bot = !bot_token.trim().is_empty();
    let has_account = match account.as_str() {
        "bot" => has_bot,
        "user" => has_user_session,
        _ => has_bot || has_user_session,
    };

    SendDefaults {
        available: module.is_some(),
        module_name: module.map(|m| m.name).unwrap_or_default(),
        chat_id: param_str(&params, "chat_id"),
        username: param_str(&params, "username"),
        topic_per_model: param_bool(&params, "topic_per_model", true),
        delete_after_send: param_bool(&params, "delete_after_send", true),
        send_video: param_bool(&params, "send_video", true),
        // 与模块保持一致：文字说明默认关闭，只上传视频
        // Match the module: the caption is off by default, so only the video is uploaded
        send_caption: param_bool(&params, "send_caption", false),
        send_cover: param_bool(&params, "send_cover", true),
        // 与模块保持一致：默认先把联系表作为图片发出去，再发视频
        // Match the module: the contact sheet is posted as an image before the video by default
        cover_first: param_bool(&params, "cover_first", true),
        batch_size: params.get("batch_size").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(3),
        account,
        has_account,
        in_pipeline,
    }
}

/// `notify_telegram` 模块保存用户会话的目录（与模块内部 `config_dir_path()` 一致）。
/// Directory where the `notify_telegram` module keeps its user session (mirrors the module's
/// own `config_dir_path()`).
fn telegram_config_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("notify_telegram"))
}

/// 判断指定 api_id 是否已有已登录的用户会话文件。
/// Whether a signed-in user session file exists for the given api_id.
pub fn user_session_exists(api_id: i32) -> bool {
    telegram_config_dir()
        .map(|d| d.join(format!("{}_user.db", api_id)).is_file())
        .unwrap_or(false)
}

/// 校验请求并合成最终的模块参数（纯函数，便于测试）。
/// Validate the request and build the final module parameters (pure, easy to test).
///
/// # 参数 / Parameters
/// - `defaults`: 流水线节点里的参数（作为默认值）/ Parameters from the pipeline node (used as defaults)
/// - `req`: 用户请求（覆盖项）/ The user request (overrides)
fn build_params(
    defaults: &HashMap<String, serde_json::Value>,
    req: &SendRequest,
) -> Result<(HashMap<String, serde_json::Value>, String), String> {
    if req.path.trim().is_empty() {
        return Err("未指定要发送的视频 / no video path given".to_string());
    }
    let video = Path::new(req.path.trim());
    if !video.is_file() {
        return Err(format!(
            "视频不存在 / video not found: {}",
            video.display()
        ));
    }

    let mut params = defaults.clone();

    // 字符串覆盖：空字符串或纯空白视为「不覆盖」，避免清空流水线里的配置
    // String overrides: empty/blank means "no override", so pipeline config is never erased
    for (key, value) in [
        ("chat_id", &req.chat_id),
        ("username", &req.username),
    ] {
        if let Some(v) = value
            && !v.trim().is_empty()
        {
            params.insert(key.to_string(), serde_json::Value::String(v.trim().to_string()));
        }
    }
    for (key, value) in [
        ("topic_per_model", req.topic_per_model),
        ("delete_after_send", req.delete_after_send),
        ("send_video", req.send_video),
        ("send_caption", req.send_caption),
        ("send_cover", req.send_cover),
        ("cover_first", req.cover_first),
    ] {
        if let Some(v) = value {
            params.insert(key.to_string(), serde_json::Value::Bool(v));
        }
    }
    if let Some(v) = req.batch_size {
        params.insert(
            "batch_size".to_string(),
            serde_json::Value::Number(serde_json::Number::from(v)),
        );
    }

    // 最终生效的群组 ID（用于任务记录与前端展示）
    // The chat id that will actually be used (for the task record and the UI)
    let chat_id = param_str(&params, "chat_id");
    if chat_id.is_empty() {
        return Err(
            "未配置群组 ID：请在后处理节点或发送对话框里填写 / no chat id configured: fill it in \
             on the pipeline node or in the send dialog"
                .to_string(),
        );
    }

    Ok((params, chat_id))
}

/// 手动发送的核心实现（同步，在阻塞线程中调用）。
/// 与后处理共享同一条串行锁，避免与自动流水线同时上传同一个文件。
///
/// Core implementation of a manual send (synchronous, called from a blocking thread).
/// Shares the post-processing serial lock so it never uploads the same file as the
/// automatic pipeline at the same time.
pub fn send_inner(req: SendRequest, emitter: &Arc<dyn Emitter>, state: &Arc<AppState>) {
    let path_str = req.path.trim().to_string();

    let (params, chat_id) = match build_params(&pipeline_params(state), &req) {
        Ok(v) => v,
        Err(e) => {
            // 还没有任务记录时也要建一条，界面才能显示失败原因
            // Create a record even when nothing started, so the UI can show why it failed
            state.tg_task_fail(
                &path_str,
                &e,
                req.chat_id.as_deref().unwrap_or(""),
                req.delete_after_send.unwrap_or(true),
            );
            emitter.emit(
                "telegram-send-done",
                &serde_json::json!({ "path": path_str, "success": false, "message": e }),
            );
            return;
        }
    };

    let delete_after_send = param_bool(&params, "delete_after_send", true);

    let module = match find_module() {
        Some(m) => m,
        None => {
            let msg = format!(
                "未找到 Telegram 模块（modules/{}）/ Telegram module not found",
                TELEGRAM_MODULE_ID
            );
            state.tg_task_fail(&path_str, &msg, &chat_id, delete_after_send);
            emitter.emit(
                "telegram-send-done",
                &serde_json::json!({ "path": path_str, "success": false, "message": msg }),
            );
            return;
        }
    };

    // 登记任务；已在发送中则直接忽略（前端不会重复点击）
    // Register the task; ignore when the same file is already being sent
    if !state.tg_task_start(&path_str, &chat_id, delete_after_send) {
        return;
    }

    let settings = state.get_settings();
    let cancel = state.tg_task_make_cancel_flag(&path_str);

    emitter.emit(
        "telegram-send-started",
        &serde_json::json!({ "path": path_str, "chatId": chat_id }),
    );

    // 并发上传：同一时刻最多 batch_size 个上传，其余排队（发送对话框里可调）。
    // 正在运行的上传不会被压缩；取消排队中的任务立即生效。
    // Concurrent uploads: at most `batch_size` at once, the rest queue (set in the send dialog).
    // In-flight uploads are never squeezed; cancelling a queued task takes effect immediately.
    if let Some(n) = req.batch_size {
        state.tg_gate.set_limit(n.max(1) as usize);
    }

    state.tg_task_status_text(&path_str, "Queued (waiting for upload slot)");
    emitter.emit(
        "telegram-send-progress",
        &serde_json::json!({ "path": path_str, "statusText": "Queued (waiting for upload slot)" }),
    );

    let _permit = match state.tg_gate.acquire(&cancel) {
        Some(p) => p,
        None => {
            state.tg_task_finish(&path_str, "cancelled", "cancelled");
            emitter.emit(
                "telegram-send-done",
                &serde_json::json!({
                    "path": path_str,
                    "success": false,
                    "cancelled": true,
                    "message": "cancelled",
                }),
            );
            return;
        }
    };

    state.tg_task_status_text(&path_str, "Starting upload...");
    emitter.emit(
        "telegram-send-progress",
        &serde_json::json!({ "path": path_str, "statusText": "Starting upload..." }),
    );

    let video = std::path::PathBuf::from(&path_str);
    let result = run_module_once(
        &module,
        &params,
        &video,
        Some(Arc::clone(&cancel)),
        &settings,
        &|done: u32, total: u32| {
            if total == 0 {
                return;
            }
            let pct = ((done as f64) * 100.0 / (total as f64) * 100.0).round() / 100.0;
            state.tg_task_progress(&path_str, pct.min(100.0));
            emitter.emit(
                "telegram-send-progress",
                &serde_json::json!({ "path": path_str, "pct": pct.min(100.0) }),
            );
        },
        &|status: &str| {
            state.tg_task_status_text(&path_str, status);
            emitter.emit(
                "telegram-send-progress",
                &serde_json::json!({ "path": path_str, "statusText": status }),
            );
        },
        &|stream: &str, line: &str| {
            emitter.emit(
                "telegram-send-log",
                &serde_json::json!({
                    "path": path_str,
                    "stream": stream,
                    "line": line,
                }),
            );
        },
    );

    let cancelled = cancel.load(Ordering::Relaxed);

    if result.success {
        // 上传成功：按设置清空本地数据（成片、分片、元数据、副产物）
        // Upload succeeded: wipe the local data as configured
        if result.delete_input {
            let report = crate::recording::meta::purge_recording(&settings, &video);
            {
                let mut data = state.data.write();
                data.pp_results.retain(|p| p != &path_str);
            }
            let _ = state.save();
            emitter.emit(
                "recording-purged",
                &serde_json::json!({
                    "path": path_str,
                    "removed": report.removed,
                    "freedBytes": report.freed_bytes,
                    "failed": report.failed,
                    "reason": "manual-upload",
                }),
            );
        }
        state.tg_task_finish(&path_str, "done", &result.message);
        emitter.emit(
            "telegram-send-done",
            &serde_json::json!({
                "path": path_str,
                "success": true,
                "message": result.message,
                "deleted": result.delete_input,
            }),
        );
    } else {
        let status = if cancelled { "cancelled" } else { "error" };
        state.tg_task_finish(&path_str, status, &result.message);
        emitter.emit(
            "telegram-send-done",
            &serde_json::json!({
                "path": path_str,
                "success": false,
                "cancelled": cancelled,
                "message": result.message,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时视频文件，返回路径 / Create a temp video file and return its path
    fn temp_video(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, b"x").expect("write temp video");
        p
    }

    /// 构造流水线默认参数 / Build pipeline default parameters
    fn defaults(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn empty_path_is_rejected() {
        let err = build_params(&HashMap::new(), &SendRequest::default()).unwrap_err();
        assert!(err.contains("未指定") || err.contains("no video path"));
    }

    #[test]
    fn missing_video_is_rejected() {
        let req = SendRequest {
            path: "/nonexistent/video.mp4".to_string(),
            ..Default::default()
        };
        let err = build_params(&HashMap::new(), &req).unwrap_err();
        assert!(err.contains("视频不存在") || err.contains("video not found"));
    }

    #[test]
    fn overrides_win_over_pipeline_defaults() {
        let d = defaults(&[
            ("chat_id", serde_json::json!("-100111")),
            ("topic_per_model", serde_json::json!(true)),
            ("delete_after_send", serde_json::json!(true)),
        ]);
        let tmp = temp_video("tg_cmd_override_test.mp4");
        let req = SendRequest {
            path: tmp.to_string_lossy().to_string(),
            chat_id: Some("-100222".to_string()),
            topic_per_model: Some(false),
            delete_after_send: Some(false),
            ..Default::default()
        };
        let (merged, chat_id) = build_params(&d, &req).expect("params");
        assert_eq!(chat_id, "-100222", "request override must win");
        assert_eq!(
            merged.get("topic_per_model"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            merged.get("delete_after_send"),
            Some(&serde_json::Value::Bool(false))
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn missing_chat_id_is_reported() {
        let tmp = temp_video("tg_cmd_no_chat_test.mp4");
        let req = SendRequest {
            path: tmp.to_string_lossy().to_string(),
            ..Default::default()
        };
        let err = build_params(&HashMap::new(), &req).unwrap_err();
        assert!(err.contains("群组 ID") || err.contains("chat id"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn blank_overrides_do_not_erase_defaults() {
        let d = defaults(&[
            ("chat_id", serde_json::json!("-100111")),
            ("username", serde_json::json!("mygroup")),
        ]);
        let tmp = temp_video("tg_cmd_blank_test.mp4");
        let req = SendRequest {
            path: tmp.to_string_lossy().to_string(),
            chat_id: Some("   ".to_string()),
            username: Some(String::new()),
            ..Default::default()
        };
        let (merged, chat_id) = build_params(&d, &req).expect("params");
        assert_eq!(chat_id, "-100111");
        assert_eq!(param_str(&merged, "username"), "mygroup");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn chat_id_is_trimmed() {
        let d = defaults(&[("chat_id", serde_json::json!("-100111"))]);
        let tmp = temp_video("tg_cmd_trim_test.mp4");
        let req = SendRequest {
            path: format!("  {}  ", tmp.to_string_lossy()),
            chat_id: Some("  -100333  ".to_string()),
            ..Default::default()
        };
        let (_, chat_id) = build_params(&d, &req).expect("params");
        assert_eq!(chat_id, "-100333");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn boolean_params_accept_strings_and_bools() {
        let d = defaults(&[
            ("delete_after_send", serde_json::json!("true")),
            ("topic_per_model", serde_json::json!(false)),
        ]);
        assert!(param_bool(&d, "delete_after_send", false));
        assert!(!param_bool(&d, "topic_per_model", true));
        // 缺省时回落到 fallback / Falls back when absent
        assert!(param_bool(&d, "missing", true));
        assert!(!param_bool(&d, "missing", false));
    }

    #[test]
    fn delete_after_send_defaults_to_true() {
        // 用户要求：上传成功后默认删除服务器上的数据
        // Requested behaviour: data is deleted after a successful upload by default
        assert!(param_bool(&HashMap::new(), "delete_after_send", true));
        let d = send_defaults_from(&HashMap::new());
        assert!(d.delete_after_send);
    }

    /// 用给定的流水线参数计算界面默认值（不触碰磁盘配置）。
    /// Compute the UI defaults from the given pipeline parameters (no disk config involved).
    fn send_defaults_from(params: &HashMap<String, serde_json::Value>) -> SendDefaults {
        SendDefaults {
            available: true,
            module_name: String::new(),
            chat_id: param_str(params, "chat_id"),
            username: param_str(params, "username"),
            topic_per_model: param_bool(params, "topic_per_model", true),
            delete_after_send: param_bool(params, "delete_after_send", true),
            send_video: param_bool(params, "send_video", true),
            send_caption: param_bool(params, "send_caption", false),
            send_cover: param_bool(params, "send_cover", true),
            cover_first: param_bool(params, "cover_first", true),
            account: "auto".to_string(),
            has_account: false,
            in_pipeline: !params.is_empty(),
            batch_size: params.get("batch_size").and_then(|v| v.as_u64()).unwrap_or(3) as u32,
        }
    }

    #[test]
    fn defaults_reflect_the_pipeline_node() {
        let d = send_defaults_from(&defaults(&[
            ("chat_id", serde_json::json!("-100999")),
            ("delete_after_send", serde_json::json!(false)),
        ]));
        assert!(d.in_pipeline);
        assert_eq!(d.chat_id, "-100999");
        assert!(!d.delete_after_send);
    }

    #[test]
    fn send_request_accepts_camel_case_overrides() {
        // 前端发送 camelCase 字段 / The frontend sends camelCase fields
        let req: SendRequest = serde_json::from_value(serde_json::json!({
            "path": "/tmp/x.mp4",
            "chatId": "-1001",
            "topicPerModel": false,
            "deleteAfterSend": true,
            "sendVideo": false,
            "sendCaption": true,
            "sendCover": false,
            "coverFirst": false,
        }))
        .expect("deserialize");
        assert_eq!(req.chat_id.as_deref(), Some("-1001"));
        assert_eq!(req.topic_per_model, Some(false));
        assert_eq!(req.delete_after_send, Some(true));
        assert_eq!(req.send_video, Some(false));
        assert_eq!(req.send_caption, Some(true));
        assert_eq!(req.send_cover, Some(false));
        assert_eq!(req.cover_first, Some(false));
    }

    /// 文字说明默认关闭：节点里没有这个键时必须按「不发文字」处理，
    /// 否则界面会显示已关闭、模块却照旧发文字。
    ///
    /// The caption defaults to off: a node without the key must be treated as "no text",
    /// otherwise the UI would show it off while the module still posted a caption.
    #[test]
    fn caption_defaults_to_off() {
        assert!(!param_bool(&HashMap::new(), "send_caption", false));
        let d = send_defaults_from(&HashMap::new());
        assert!(!d.send_caption, "caption must be off by default");
        assert!(d.send_cover, "the cover image stays on by default");
        assert!(d.send_video, "the video stays on by default");
    }

    /// 对话框里的「只发视频」组合必须原样传给模块。
    /// The dialog's "video only" combination must reach the module unchanged.
    #[test]
    fn video_only_overrides_reach_the_module() {
        let d = defaults(&[("chat_id", serde_json::json!("-100111"))]);
        let tmp = temp_video("tg_cmd_video_only_test.mp4");
        let req = SendRequest {
            path: tmp.to_string_lossy().to_string(),
            send_video: Some(true),
            send_caption: Some(false),
            send_cover: Some(false),
            ..Default::default()
        };
        let (merged, _) = build_params(&d, &req).expect("params");
        assert_eq!(merged.get("send_video"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(
            merged.get("send_caption"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(merged.get("send_cover"), Some(&serde_json::Value::Bool(false)));
        let _ = std::fs::remove_file(&tmp);
    }

    /// 封面图默认先单独发一条：界面的默认值必须和模块一致，否则用户看到的开关是反的。
    /// The cover is posted on its own first by default: the UI default must match the module,
    /// otherwise the switch shows the opposite of what happens.
    #[test]
    fn cover_first_defaults_to_true() {
        assert!(param_bool(&HashMap::new(), "cover_first", true));
        let d = send_defaults_from(&HashMap::new());
        assert!(d.cover_first, "the cover goes out first by default");
    }

    /// 对话框里关掉「先发封面图」必须传到模块，回到「封面图放进相册」的旧行为。
    /// Turning "cover first" off in the dialog must reach the module and restore the old
    /// "cover inside the album" behaviour.
    #[test]
    fn cover_first_override_reaches_the_module() {
        let d = defaults(&[("chat_id", serde_json::json!("-100222"))]);
        let tmp = temp_video("tg_cmd_cover_first_test.mp4");
        let req = SendRequest {
            path: tmp.to_string_lossy().to_string(),
            cover_first: Some(false),
            ..Default::default()
        };
        let (merged, _) = build_params(&d, &req).expect("params");
        assert_eq!(
            merged.get("cover_first"),
            Some(&serde_json::Value::Bool(false))
        );
        // 没提到的参数保持节点里的值 / Parameters not mentioned keep the node's value
        assert_eq!(merged.get("send_cover"), None);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 刷新 Telegram 会话（不重新登录，不输入验证码）。
///
/// Spawns the notify_telegram module in `--refresh` mode: it re-opens the
/// existing session file, re-syncs the auth key and peer cache, and exits.
/// No phone number / code is needed (the session must already be authorized).
///
/// 返回 (success, stdout, stderr) / Returns (success, stdout, stderr).
pub fn run_refresh(exe: &std::path::Path, api_id: &str, proxy: &str) -> (bool, String, String) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.arg("--refresh");
    if !api_id.is_empty() {
        cmd.arg(api_id);
    }
    cmd.env("PP_API_ID", api_id);
    cmd.env("PP_PARAM_API_ID", api_id);
    if !proxy.is_empty() {
        cmd.env("PP_PROXY", proxy);
        cmd.env("PP_PARAM_PROXY", proxy);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, String::new(), format!("failed to spawn: {}", e)),
    };

    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().flatten() {
            stdout.push_str(&line);
            stdout.push('\n');
        }
    }
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().flatten() {
            stderr.push_str(&line);
            stderr.push('\n');
        }
    }

    let status = child.wait();
    let ok = status.is_ok_and(|s| s.success());
    (ok, stdout, stderr)
}
