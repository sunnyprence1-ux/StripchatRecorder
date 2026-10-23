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
fn pipeline_params(state: &AppState) -> HashMap<String, serde_json::Value> {
    let pipeline = state.get_pipeline();
    pipeline
        .nodes
        .iter()
        .find(|n| n.module_id == TELEGRAM_MODULE_ID)
        .map(|n| n.params.clone())
        .unwrap_or_default()
}

/// 取参数中的字符串值 / Read a string parameter value
fn param_str(params: &HashMap<String, serde_json::Value>, key: &str) -> String {
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
    ] {
        if let Some(v) = value {
            params.insert(key.to_string(), serde_json::Value::Bool(v));
        }
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

    // 串行上传：多个大文件同时上传会把上行带宽切碎，也会让 Telegram 触发限流
    // Serialise uploads: concurrent large files fragment the uplink and trip Telegram's rate limits
    let _guard = state.tg_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 排队期间可能已被取消 / The task may have been cancelled while queued
    if cancel.load(Ordering::Relaxed) {
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
            account: "auto".to_string(),
            has_account: false,
            in_pipeline: !params.is_empty(),
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
        }))
        .expect("deserialize");
        assert_eq!(req.chat_id.as_deref(), Some("-1001"));
        assert_eq!(req.topic_per_model, Some(false));
        assert_eq!(req.delete_after_send, Some(true));
        assert_eq!(req.send_video, Some(false));
        assert_eq!(req.send_caption, Some(true));
        assert_eq!(req.send_cover, Some(false));
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
}
