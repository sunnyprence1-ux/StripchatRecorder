//! 后处理流水线引擎 / Post-processing Pipeline Engine
//!
//! 负责发现 modules/ 目录中的后处理模块、执行流水线节点，以及管理模块进程的生命周期。
//! 每个模块是一个独立的可执行文件，通过环境变量接收参数，通过 stdout 上报进度和输出路径。
//!
//! Responsible for discovering post-processing modules in the modules/ directory,
//! executing pipeline nodes, and managing module process lifecycles.
//! Each module is a standalone executable that receives parameters via environment variables
//! and reports progress and output paths via stdout.

use crate::config::settings::exe_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 模块参数定义（从 `--describe` 输出中反序列化）/ Module parameter definition (deserialized from `--describe` output)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamDef {
    /// 参数键名 / Parameter key
    pub key: String,
    /// 参数显示标签 / Parameter display label
    pub label: String,
    /// 参数类型（"string" / "number" / "boolean" / "select"）/ Parameter type
    pub r#type: String,
    /// 参数默认值 / Parameter default value
    pub default: serde_json::Value,
    /// select 类型的可选项 / Options for select type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// 后处理模块信息（从 `--describe` 输出中反序列化）/ Post-processing module info (deserialized from `--describe` output)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleInfo {
    /// 模块唯一 ID / Module unique ID
    pub id: String,
    /// 模块显示名称 / Module display name
    pub name: String,
    /// 模块功能描述 / Module description
    pub description: String,
    /// 模块参数定义列表 / Module parameter definitions
    pub params: Vec<ParamDef>,
    /// 多语言翻译（可选，key 为语言代码如 "en-US"）/ i18n translations (optional, key is locale like "en-US")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i18n: Option<serde_json::Value>,
    /// 是否支持自检模式（通过 `PP_TEST=1` 调用，无需输入视频）
    /// Whether the module supports self-test mode (invoked with `PP_TEST=1`, no input video needed)
    #[serde(default)]
    pub testable: bool,
    /// 是否支持交互式登录（通过 `--login` 调用，用 stdin/stdout 行协议驱动）
    /// Whether the module supports interactive login (invoked with `--login`, driven over a
    /// stdin/stdout line protocol)
    #[serde(default)]
    pub loginable: bool,
    /// 模块可执行文件路径（不序列化，运行时填充）/ Module executable path (not serialized, filled at runtime)
    #[serde(skip)]
    pub exe_path: PathBuf,
}

/// 流水线节点（模块实例）/ Pipeline node (module instance)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineNode {
    /// 节点唯一 ID（UUID）/ Node unique ID (UUID)
    pub node_id: String,
    /// 对应的模块 ID / Corresponding module ID
    pub module_id: String,
    /// 节点参数值 / Node parameter values
    pub params: HashMap<String, serde_json::Value>,
    /// 是否启用此节点 / Whether this node is enabled
    pub enabled: bool,
}

/// 流水线配置 / Pipeline configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {
    /// 有序的节点列表 / Ordered list of nodes
    pub nodes: Vec<PipelineNode>,
}

/// 返回模块可执行文件所在目录（可执行文件同目录下的 modules/ 文件夹）。
/// Returns the modules directory (modules/ folder next to the executable).
pub fn modules_dir() -> PathBuf {
    exe_dir().join("modules")
}

/// 扫描 modules/ 目录，发现所有可用的后处理模块。
/// 对每个可执行文件调用 `--describe` 获取模块元数据。
///
/// Scan the modules/ directory to discover all available post-processing modules.
/// Calls `--describe` on each executable to get module metadata.
pub fn discover_modules() -> Vec<ModuleInfo> {
    let dir = modules_dir();
    if !dir.exists() {
        return vec![];
    }

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut modules = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // 平台相关的可执行文件检测 / Platform-specific executable detection
        #[cfg(target_os = "windows")]
        let is_exec = path.extension().and_then(|e| e.to_str()) == Some("exe");
        #[cfg(not(target_os = "windows"))]
        let is_exec = {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        };

        if !is_exec {
            continue;
        }

        match describe_module(&path) {
            Ok(mut info) => {
                info.exe_path = path;
                modules.push(info);
            }
            Err(e) => {
                tracing::error!("Failed to describe module {:?}: {}", path, e);
            }
        }
    }

    modules
}

/// 调用模块可执行文件的 `--describe` 参数，解析并返回模块元数据。
/// Call the module executable with `--describe` and parse the returned module metadata.
fn describe_module(exe: &PathBuf) -> crate::core::error::Result<ModuleInfo> {
    let output = std::process::Command::new(exe)
        .arg("--describe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| crate::core::error::AppError::Other(format!("spawn: {}", e)))?;

    if !output.status.success() {
        return Err(crate::core::error::AppError::Other(format!(
            "exit {}",
            output.status
        )));
    }

    let info: ModuleInfo = serde_json::from_slice(&output.stdout)
        .map_err(|e| crate::core::error::AppError::Other(format!("json: {}", e)))?;

    Ok(info)
}

/// 单个节点的执行结果 / Execution result of a single node
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeResult {
    /// 节点 ID / Node ID
    pub node_id: String,
    /// 模块 ID / Module ID
    pub module_id: String,
    /// 是否执行成功 / Whether execution succeeded
    pub success: bool,
    /// 结果消息（成功时为最后一行 stdout，失败时为错误信息）/ Result message (last stdout line on success, error message on failure)
    pub message: String,
    /// 模块输出的文件路径（从 `OUTPUT:` 前缀行解析，不序列化）/ Module output file path (parsed from `OUTPUT:` prefix line, not serialized)
    #[serde(skip)]
    pub output: Option<PathBuf>,
    /// 模块是否请求主程序删除其输入文件（从 `DELETE_INPUT` 协议行解析，不序列化）
    /// Whether the module requested the host to delete its input file (parsed from `DELETE_INPUT` protocol line, not serialized)
    #[serde(skip)]
    pub delete_input: bool,
}

/// 执行后处理流水线，依次运行所有启用的节点。
/// 每个节点的输出路径作为下一个节点的输入；若节点无输出（如 filter_short 删除文件），流水线终止。
///
/// Execute the post-processing pipeline, running all enabled nodes in sequence.
/// Each node's output path becomes the next node's input; if a node has no output
/// (e.g., filter_short deletes the file), the pipeline terminates.
///
/// # 参数 / Parameters
/// - `video_path`: 初始输入视频路径 / Initial input video path
/// - `pipeline`: 流水线配置 / Pipeline configuration
/// - `modules`: 可用模块列表 / Available module list
/// - `cancel`: 可选的取消标志 / Optional cancel flag
/// - `max_tmp_dir_gb`: tmp 目录最大占用（GB，0 = 不限制）/ Max tmp dir size in GB (0 = unlimited)
/// - `on_progress`: 进度回调（节点完成数, 总节点数, 模块内进度, 模块总进度, 模块名, 状态文字）/ Progress callback
/// - `on_log`: 日志回调（模块 ID, 流名称, 行内容）/ Log callback
/// - `on_delete_input`: 模块请求删除输入时的回调（由调用方执行完整清理）
///   Callback invoked when a module requests deletion of its input (the caller performs the
///   full cleanup). The pipeline stops afterwards when the node's output is that same file,
///   since there is nothing left to feed the next node.
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    modules: &[ModuleInfo],
    cancel: Option<Arc<AtomicBool>>,
    settings: &crate::config::settings::Settings,
    on_progress: impl Fn(usize, usize, u32, u32, &str, &str),
    on_log: impl Fn(&str, &str, &str),
    on_delete_input: impl Fn(&std::path::Path),
) -> Vec<NodeResult> {
    let mut results = Vec::new();
    let mut current_input = video_path.to_path_buf();
    let max_tmp_dir_gb = settings.max_tmp_dir_gb;
    // 副产物目录：与元数据文件同目录（输出目录），避免污染最终视频目录
    // Sidecar dir: same directory as the metadata file (output dir), keeping the video dir clean
    let sidecar_dir = settings.sidecar_dir_for_video(video_path);

    let all_nodes: Vec<&PipelineNode> = pipeline.nodes.iter().collect();
    let total = all_nodes.iter().filter(|n| n.enabled).count();
    let mut done = 0usize;

    for node in all_nodes.iter() {
        if !node.enabled {
            continue;
        }

        // 检查取消标志 / Check cancel flag
        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }

        let module = match modules.iter().find(|m| m.id == node.module_id) {
            Some(m) => m,
            None => {
                // 模块缺失，终止整条流水线 / Module missing, abort the entire pipeline
                results.push(NodeResult {
                    node_id: node.node_id.clone(),
                    module_id: node.module_id.clone(),
                    success: false,
                    message: format!("模块 '{}' 不存在，请检查 modules/ 目录", node.module_id),
                    output: None,
                    delete_input: false,
                });
                done += 1;
                on_progress(done, total, 0, 0, &node.module_id, "");
                break;
            }
        };

        let cur_done = done;
        let module_name = module.name.clone();
        let module_id_for_log = node.module_id.clone();
        let status_text = std::sync::Mutex::new(String::new());
        let last_mod = std::sync::Mutex::new((0u32, 0u32));
        let result = run_node(
            module,
            node,
            &current_input,
            cancel.clone(),
            max_tmp_dir_gb,
            sidecar_dir.as_deref(),
            &|md, mt| {
                *last_mod.lock().unwrap() = (md, mt);
                let st = status_text.lock().unwrap().clone();
                on_progress(cur_done, total, md, mt, &module_name, &st);
            },
            &|stream, line| {
                on_log(&module_id_for_log, stream, line);
            },
            &|st| {
                *status_text.lock().unwrap() = st.to_string();
                let (md, mt) = *last_mod.lock().unwrap();
                on_progress(cur_done, total, md, mt, &module_name, st);
            },
        );

        done += 1;
        on_progress(done, total, 0, 0, &module_name, "");

        // 若模块请求删除其输入文件，由主程序执行完整清理（成片、分片、元数据、副产物）
        // If the module requested deletion of its input file, the host performs the full
        // cleanup (video, segments, metadata, side outputs)
        if result.delete_input {
            on_delete_input(&current_input);
        }

        match &result.output {
            Some(out) => {
                // 输入已被删除且节点输出就是该输入：后面没有可用的文件，流水线结束
                // The input was deleted and the node's output is that same file: nothing is
                // left for the next node, so the pipeline ends here
                if result.delete_input && out == &current_input {
                    results.push(result);
                    break;
                }
                // 节点有输出，继续执行下一个节点 / Node has output, continue to next node
                current_input = out.clone();
                results.push(result);
            }
            None if result.success => {
                // 节点成功但无输出（模块已请求删除输入），终止流水线
                // Node succeeded but has no output (module requested input deletion), terminate pipeline
                results.push(result);
                break;
            }
            None => {
                // 节点失败，终止流水线 / Node failed, terminate pipeline
                results.push(result);
                break;
            }
        }
    }

    results
}

/// 执行单个流水线节点（启动子进程，读取 stdout/stderr，处理取消）。
/// Execute a single pipeline node (spawn subprocess, read stdout/stderr, handle cancellation).
///
/// # 参数 / Parameters
/// - `module`: 模块信息（含可执行文件路径）/ Module info (including executable path)
/// - `node`: 节点配置（含参数）/ Node configuration (including parameters)
/// - `input`: 输入文件路径 / Input file path
/// - `cancel`: 可选的取消标志 / Optional cancel flag
/// - `max_tmp_dir_gb`: tmp 目录最大占用（GB，0 = 不限制）/ Max tmp dir size in GB (0 = unlimited)
/// - `on_module_progress`: 模块内进度回调 / Module-level progress callback
/// - `on_log`: 日志行回调 / Log line callback
/// - `on_status`: 状态文字回调（来自 `STATUS:` 前缀行）/ Status text callback (from `STATUS:` prefix lines)
#[allow(clippy::too_many_arguments)]
fn run_node(
    module: &ModuleInfo,
    node: &PipelineNode,
    input: &std::path::Path,
    cancel: Option<Arc<AtomicBool>>,
    max_tmp_dir_gb: f64,
    sidecar_dir: Option<&std::path::Path>,
    on_module_progress: &dyn Fn(u32, u32),
    on_log: &dyn Fn(&str, &str),
    on_status: &dyn Fn(&str),
) -> NodeResult {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    /// 子进程输出流事件 / Subprocess output stream events
    enum StreamEvent {
        StdoutLine(String),
        StderrLine(String),
        StdoutEof,
        StderrEof,
    }

    let mut cmd = std::process::Command::new(&module.exe_path);
    // PP_MAX_TMP_MB 以 MB 为单位传给模块（GB * 1024，向下取整）
    // Pass PP_MAX_TMP_MB to the module in MB (GB * 1024, truncated)
    let max_tmp_mb = (max_tmp_dir_gb * 1024.0) as u64;
    cmd.env("PP_INPUT", input)
        .env("PP_EXE_DIR", exe_dir())
        .env("PP_MAX_TMP_MB", max_tmp_mb.to_string());

    // 副产物（联系表图片等）输出目录：默认与输入同目录，分离布局下为输出目录
    // Side-output directory (contact sheets etc.): input dir by default, output dir in split layout
    if let Some(dir) = sidecar_dir {
        cmd.env("PP_SIDECAR_DIR", dir);
    }

    cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // 将节点参数转换为 PP_PARAM_{KEY} 环境变量 / Convert node params to PP_PARAM_{KEY} env vars
    for (key, val) in &node.params {
        let env_key = format!("PP_PARAM_{}", key.to_uppercase());
        let env_val = match val {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        cmd.env(env_key, env_val);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return NodeResult {
                node_id: node.node_id.clone(),
                module_id: node.module_id.clone(),
                success: false,
                message: format!("Failed to spawn: {}", e),
                output: None,
                delete_input: false,
            }
        }
    };

    let mut last_message = String::new();
    let mut stderr_msg = String::new();
    let mut panic_msg = String::new();
    let mut output_path: Option<PathBuf> = None;
    let mut delete_input = false;
    let mut cancelled = false;

    let module_id = &node.module_id;

    // 使用 channel 将 stdout/stderr 的行事件汇聚到主循环
    // Use a channel to funnel stdout/stderr line events into the main loop
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let mut stdout_done = true;
    let mut stderr_done = true;

    if let Some(stdout) = child.stdout.take() {
        stdout_done = false;
        let tx_stdout = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx_stdout.send(StreamEvent::StdoutLine(line)).is_err() {
                    return;
                }
            }
            let _ = tx_stdout.send(StreamEvent::StdoutEof);
        });
    }

    if let Some(stderr) = child.stderr.take() {
        stderr_done = false;
        let tx_stderr = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx_stderr.send(StreamEvent::StderrLine(line)).is_err() {
                    return;
                }
            }
            let _ = tx_stderr.send(StreamEvent::StderrEof);
        });
    }

    drop(tx);

    while !(stdout_done && stderr_done) {
        // 每 100ms 检查一次取消标志 / Check cancel flag every 100ms
        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            // Windows 上使用 taskkill 强制终止进程树 / Use taskkill on Windows to force-kill the process tree
            #[cfg(target_os = "windows")]
            {
                let pid = child.id();
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            cancelled = true;
            break;
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::StdoutLine(line)) => {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("PROGRESS:") {
                    // 解析 PROGRESS:{done}/{total} 格式 / Parse PROGRESS:{done}/{total} format
                    let mut parts = rest.splitn(2, '/');
                    if let (Some(d), Some(t)) = (parts.next(), parts.next())
                        && let (Ok(done), Ok(total)) =
                            (d.trim().parse::<u32>(), t.trim().parse::<u32>())
                    {
                        on_module_progress(done, total);
                    }
                } else if let Some(status_text) = trimmed.strip_prefix("STATUS:") {
                    // 解析 STATUS:{text} 格式（上传速度等）/ Parse STATUS:{text} format (upload speed, etc.)
                    on_log("status", status_text.trim());
                    on_status(status_text.trim());
                } else if let Some(path) = trimmed.strip_prefix("OUTPUT:") {
                    // 解析 OUTPUT:{path} 格式 / Parse OUTPUT:{path} format
                    output_path = Some(PathBuf::from(path.trim()));
                } else if trimmed == "DELETE_INPUT" {
                    // 模块请求主程序删除其输入文件 / Module requests host to delete its input file
                    delete_input = true;
                } else if !trimmed.is_empty() {
                    tracing::info!("[{}] {}", module_id, trimmed);
                    on_log("stdout", trimmed);
                    last_message = trimmed.to_string();
                }
            }
            Ok(StreamEvent::StderrLine(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // 过滤 Rust panic 的 BACKTRACE 提示行 / Filter Rust panic BACKTRACE hint lines
                if trimmed.starts_with("note: run with `RUST_BACKTRACE") {
                    continue;
                }
                if trimmed.contains("panicked at") && panic_msg.is_empty() {
                    panic_msg = trimmed.to_string();
                }
                tracing::warn!("[{}] stderr: {}", module_id, trimmed);
                on_log("stderr", trimmed);
                stderr_msg = trimmed.to_string();
            }
            Ok(StreamEvent::StdoutEof) => {
                stdout_done = true;
            }
            Ok(StreamEvent::StderrEof) => {
                stderr_done = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stdout_done = true;
                stderr_done = true;
            }
        }
    }

    if cancelled {
        let _ = child.wait();
        return NodeResult {
            node_id: node.node_id.clone(),
            module_id: node.module_id.clone(),
            success: false,
            message: "cancelled".to_string(),
            output: None,
            delete_input: false,
        };
    }

    // panic 消息优先于普通 stderr 消息 / Panic message takes priority over regular stderr message
    if !panic_msg.is_empty() {
        stderr_msg = panic_msg;
    }

    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            return NodeResult {
                node_id: node.node_id.clone(),
                module_id: node.module_id.clone(),
                success: false,
                message: format!("wait failed: {}", e),
                output: None,
                delete_input: false,
            }
        }
    };

    let success = status.success();
    let message = if success {
        if last_message.is_empty() {
            "OK".to_string()
        } else {
            last_message
        }
    } else if !stderr_msg.is_empty() {
        stderr_msg
    } else if !last_message.is_empty() {
        last_message
    } else {
        format!("exit {}", status)
    };

    NodeResult {
        node_id: node.node_id.clone(),
        module_id: node.module_id.clone(),
        success,
        message,
        output: if success { output_path } else { None },
        delete_input: success && delete_input,
    }
}

// ─── 单个模块的一次性执行 / One-off execution of a single module ─────────────

/// 单独运行一个模块一次（不经过流水线），用于「手动发送到 Telegram」这类按需操作。
/// 参数合并后的 `PP_PARAM_*`、进度协议、`OUTPUT:` / `DELETE_INPUT` 解析与流水线节点完全一致。
///
/// Run a single module once, outside the pipeline. Used for on-demand actions such as
/// "send to Telegram manually". Parameter mapping, progress protocol and `OUTPUT:` /
/// `DELETE_INPUT` parsing behave exactly like a pipeline node.
///
/// # 参数 / Parameters
/// - `module`: 要运行的模块 / The module to run
/// - `params`: 参数值（已合并默认值与用户覆盖值）/ Parameter values (defaults already merged with overrides)
/// - `input`: 输入文件路径 / Input file path
/// - `cancel`: 可选的取消标志 / Optional cancel flag
/// - `settings`: 用于确定副产物目录与 tmp 上限 / Used for the sidecar dir and tmp cap
/// - `on_progress`: 模块内进度回调 / Module-level progress callback
/// - `on_status`: 状态文字回调 / Status text callback
/// - `on_log`: 日志行回调 / Log line callback
#[allow(clippy::too_many_arguments)]
pub fn run_module_once(
    module: &ModuleInfo,
    params: &HashMap<String, serde_json::Value>,
    input: &std::path::Path,
    cancel: Option<Arc<AtomicBool>>,
    settings: &crate::config::settings::Settings,
    on_progress: &dyn Fn(u32, u32),
    on_status: &dyn Fn(&str),
    on_log: &dyn Fn(&str, &str),
) -> NodeResult {
    let node = PipelineNode {
        node_id: "manual".to_string(),
        module_id: module.id.clone(),
        params: params.clone(),
        enabled: true,
    };
    let sidecar = settings.sidecar_dir_for_video(input);
    run_node(
        module,
        &node,
        input,
        cancel,
        settings.max_tmp_dir_gb,
        sidecar.as_deref(),
        on_progress,
        on_log,
        on_status,
    )
}

// ─── 模块自检 / Module self-test ─────────────────────────────────────────────

/// 自检输出的一行日志 / A single line of self-test output
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestLog {
    /// 输出流名称（"stdout" / "stderr"）/ Output stream name
    pub stream: String,
    /// 行内容 / Line content
    pub line: String,
}

/// 模块自检结果 / Module self-test result
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    /// 自检是否成功 / Whether the self-test succeeded
    pub success: bool,
    /// 结果摘要（成功时为最后一行 stdout，失败时为最后一行 stderr）
    /// Result summary (last stdout line on success, last stderr line on failure)
    pub message: String,
    /// 完整输出日志（按时间顺序，上限 `TEST_MAX_LOG_LINES` 行）
    /// Full output log (chronological, capped at `TEST_MAX_LOG_LINES` lines)
    pub logs: Vec<TestLog>,
}

/// 自检最长执行时间 / Maximum self-test duration
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// 自检保留的最大日志行数 / Maximum number of log lines kept from a self-test
const TEST_MAX_LOG_LINES: usize = 500;

/// 以自检模式（`PP_TEST=1`）运行模块：不提供 `PP_INPUT`，由模块自行验证其配置与外部连通性。
///
/// Runs a module in self-test mode (`PP_TEST=1`): no `PP_INPUT` is provided; the module
/// verifies its own configuration and external connectivity.
///
/// # 参数 / Parameters
/// - `module`: 模块信息（含可执行文件路径与 `testable` 标记）/ Module info (exe path and `testable` flag)
/// - `params`: 节点参数值 / Node parameter values
pub fn run_module_test(
    module: &ModuleInfo,
    params: &HashMap<String, serde_json::Value>,
) -> TestResult {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// 子进程输出事件 / Subprocess output events
    enum TestEvent {
        Line(bool, String),
        Eof,
    }

    if !module.testable {
        return TestResult {
            success: false,
            message: format!("模块 '{}' 不支持自检", module.id),
            logs: vec![],
        };
    }

    let mut cmd = std::process::Command::new(&module.exe_path);
    cmd.env("PP_TEST", "1")
        .env("PP_EXE_DIR", exe_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, val) in params {
        let env_key = format!("PP_PARAM_{}", key.to_uppercase());
        let env_val = match val {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        cmd.env(env_key, env_val);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return TestResult {
                success: false,
                message: format!("Failed to spawn: {}", e),
                logs: vec![],
            }
        }
    };

    let (tx, rx) = mpsc::channel::<TestEvent>();
    let mut open_streams = 0usize;

    if let Some(stdout) = child.stdout.take() {
        open_streams += 1;
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(TestEvent::Line(false, line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(TestEvent::Eof);
        });
    }

    if let Some(stderr) = child.stderr.take() {
        open_streams += 1;
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(TestEvent::Line(true, line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(TestEvent::Eof);
        });
    }

    drop(tx);

    let mut logs: Vec<TestLog> = Vec::new();
    let mut truncated = false;
    let mut last_stdout = String::new();
    let started = Instant::now();
    let mut timed_out = false;

    while open_streams > 0 {
        if started.elapsed() > TEST_TIMEOUT {
            timed_out = true;
            let _ = child.kill();
            break;
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(TestEvent::Line(is_stderr, line)) => {
                let line = line.trim_end().to_string();
                if line.trim().is_empty() {
                    continue;
                }
                if logs.len() < TEST_MAX_LOG_LINES {
                    logs.push(TestLog {
                        stream: if is_stderr { "stderr" } else { "stdout" }.to_string(),
                        line: line.clone(),
                    });
                } else {
                    truncated = true;
                }
                if !is_stderr {
                    last_stdout = line;
                }
            }
            Ok(TestEvent::Eof) => open_streams = open_streams.saturating_sub(1),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait();

    if truncated {
        logs.push(TestLog {
            stream: "stdout".to_string(),
            line: format!("… output truncated ({} lines max)", TEST_MAX_LOG_LINES),
        });
    }

    let exit_ok = matches!(&status, Ok(s) if s.success());
    let success = !timed_out && exit_ok;

    let message = if timed_out {
        format!("自检超时（{} 秒）", TEST_TIMEOUT.as_secs())
    } else if success {
        if last_stdout.is_empty() {
            "OK".to_string()
        } else {
            last_stdout
        }
    } else {
        // 失败时优先展示最后一行 stderr / Prefer the last stderr line on failure
        logs.iter()
            .rev()
            .find(|l| l.stream == "stderr")
            .map(|l| l.line.clone())
            .unwrap_or_else(|| match &status {
                Ok(s) => format!("exit {}", s),
                Err(e) => format!("wait failed: {}", e),
            })
    };

    TestResult {
        success,
        message,
        logs,
    }
}
