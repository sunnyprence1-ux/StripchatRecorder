//! 应用配置与全局状态管理 / Application Configuration and Global State Management
//!
//! 定义 `Settings`（用户配置）、`AppData`（持久化数据）和 `AppState`（运行时状态）。
//! `AppState` 通过 `parking_lot::RwLock` 保护共享数据，并提供后处理任务状态跟踪。
//!
//! Defines `Settings` (user configuration), `AppData` (persisted data), and `AppState` (runtime state).
//! `AppState` protects shared data with `parking_lot::RwLock` and provides post-processing task state tracking.

use crate::core::error::{AppError, Result};
use crate::postprocess::pipeline::PipelineConfig;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Mouflon 密钥存储结构，持久化到 mouflon_keys.json。
/// Mouflon key store, persisted to mouflon_keys.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MouflonKeysStore {
    /// pkey -> pdkey 密钥对 / pkey -> pdkey key pairs
    #[serde(default)]
    pub keys: HashMap<String, String>,
    /// 数据源（Worker）的密钥更新时间（RFC 3339），同步时与 Worker 返回的 updated_at 比对，相同则跳过写入。
    /// Key update timestamp from the data source (Worker, RFC 3339).
    /// Compared against the Worker's `updated_at`; skip write if equal.
    #[serde(default)]
    pub auto_synced_at: Option<String>,
    /// 最近一次手动添加/删除密钥的时间（RFC 3339）。
    /// Timestamp of the last manual key add/remove (RFC 3339).
    #[serde(default)]
    pub manual_updated_at: Option<String>,
}

/// 后处理任务状态快照（序列化后发送给前端）。
/// Post-processing task status snapshot (serialized and sent to the frontend).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpTaskStatus {
    /// 视频文件路径 / Video file path
    pub path: String,
    /// 整体进度百分比（0.0 - 100.0）/ Overall progress percentage (0.0 - 100.0)
    pub pct: f64,
    /// 当前模块已完成进度值 / Current module done progress value
    pub mod_done: u32,
    /// 当前模块总进度值 / Current module total progress value
    pub mod_total: u32,
    /// 当前模块名称 / Current module name
    pub module_name: String,
    /// 已完成的节点数 / Number of completed nodes
    pub done: usize,
    /// 总节点数 / Total number of nodes
    pub total: usize,
    /// 任务状态字符串（"waiting" / "running" / "done" / "error"）/ Task status string
    pub status: String,
    /// 是否来自内存（true = 运行中任务，false = 持久化结果）/ Whether from memory (true = in-progress, false = persisted result)
    pub from_memory: bool,
}

/// 手动发送任务状态快照（序列化后发送给前端）。
/// Manual-send task status snapshot (serialized and sent to the frontend).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TgSendStatus {
    /// 视频文件路径 / Video file path
    pub path: String,
    /// 任务状态（"running" / "done" / "error" / "cancelled"）/ Task status
    pub status: String,
    /// 进度百分比（0.0 - 100.0）/ Progress percentage (0.0 - 100.0)
    pub pct: f64,
    /// 状态文字（上传速度等）/ Status text (upload speed etc.)
    pub status_text: String,
    /// 结果消息 / Result message
    pub message: String,
    /// 任务开始时间（RFC 3339）/ Task start time (RFC 3339)
    pub started_at: String,
    /// 成功后是否删除本地数据 / Whether local data is deleted on success
    pub delete_after_send: bool,
    /// 实际使用的群组 ID / Group id actually used
    pub chat_id: String,
}

/// 用户可配置的录制器设置 / User-configurable recorder settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 录制文件输出目录（存放 .ts 分片与合并前的会话目录）/ Recording output directory (stores .ts segments and session dirs)
    pub output_dir: String,
    /// 最终视频目录（仅存放合并后的视频与元数据；留空表示与 output_dir 相同）
    /// Final video directory (merged videos and their metadata only; empty = same as output_dir)
    #[serde(default)]
    pub final_video_dir: String,
    /// 主播状态轮询间隔（秒）/ Streamer status poll interval (seconds)
    pub poll_interval_secs: u64,
    /// 是否默认开启自动录制 / Whether auto-record is enabled by default
    pub auto_record: bool,
    /// Stripchat API 代理地址 / Stripchat API proxy URL
    pub api_proxy_url: Option<String>,
    /// CDN 分片下载代理地址 / CDN segment download proxy URL
    pub cdn_proxy_url: Option<String>,
    /// Stripchat 镜像站地址 / Stripchat mirror site URL
    pub sc_mirror_url: Option<String>,
    /// 最大并发录制数（0 = 不限制）/ Max concurrent recordings (0 = unlimited)
    pub max_concurrent: usize,
    /// Per-file recording duration in seconds (0 = unlimited).
    #[serde(default)]
    pub max_recording_duration_secs: u64,
    /// 录制片段合并格式（默认 "mp4"）/ Recording segment merge format (default "mp4")
    #[serde(default = "default_merge_format")]
    pub merge_format: String,
    /// 首选录制分辨率高度（0 = 原始/最高画质）/ Preferred recording resolution height (0 = original/highest quality)
    #[serde(default)]
    pub preferred_resolution: u32,
    /// 首选分辨率不可用时的方向（"lower" 或 "higher"）/ Fallback direction when preferred resolution is unavailable ("lower" or "higher")
    #[serde(default = "default_resolution_preference")]
    pub resolution_preference: String,
    /// 后处理临时目录最大占用（GB，0 = 不限制，默认 50 GB）
    /// Max size of the post-processing tmp directory in GB (0 = unlimited, default 50 GB)
    #[serde(default = "default_max_tmp_dir_gb")]
    pub max_tmp_dir_gb: f64,
    /// 界面语言（"zh-CN" 或 "en-US"）/ UI language ("zh-CN" or "en-US")
    #[serde(default = "default_language")]
    pub language: String,
    /// 监听端口 / Listen port
    #[serde(default = "default_server_port")]
    pub server_port: u16,
    /// Mouflon Keys 同步 Worker URL（为空则不启用自动同步）
    /// Mouflon Keys sync Worker URL (empty = auto-sync disabled)
    #[serde(default = "default_mouflon_sync_url")]
    pub mouflon_sync_url: Option<String>,
    /// Mouflon Keys 同步 Worker 鉴权 Token（对应 Worker 的 AUTH_TOKEN 环境变量）
    /// Mouflon Keys sync Worker auth token (corresponds to Worker's AUTH_TOKEN env var)
    #[serde(default)]
    pub mouflon_sync_token: Option<String>,
    /// 首次启动向导是否已完成（false = 显示 Setup 页面）
    /// Whether the first-launch setup wizard has been completed (false = show Setup page)
    #[serde(default)]
    pub setup_done: bool,
    /// 后处理并发上限（同一时刻最多运行几个后处理流水线/上传）/ Max concurrent post-processing
    /// tasks (pipelines + Telegram uploads) running at once (1 = serial, higher = parallel).
    #[serde(default = "default_pp_concurrency")]
    pub pp_concurrency: usize,
}

/// Mouflon 同步地址的默认值 / Default value for Mouflon sync URL
fn default_mouflon_sync_url() -> Option<String> {
    Some("https://mouflon.chantrail.com".to_string())
}

/// 合并格式的默认值 / Default value for merge format
fn default_merge_format() -> String {
    "mp4".to_string()
}

/// 分辨率回退方向的默认值 / Default resolution fallback direction
fn default_resolution_preference() -> String {
    "lower".to_string()
}

/// tmp 目录最大占用的默认值（50 GB）/ Default value for max tmp dir size (50 GB)
fn default_max_tmp_dir_gb() -> f64 {
    50.0
}

/// 语言的默认值 / Default value for language
fn default_language() -> String {
    "zh-CN".to_string()
}

/// Server 端口的默认值 / Default value for server port
fn default_server_port() -> u16 {
    3030
}

/// 后处理并发的默认值（3）/ Default post-processing concurrency
fn default_pp_concurrency() -> usize {
    3
}

/// 返回可执行文件所在目录，用于定位配置文件和模块目录。
///
/// `current_exe()` 会解析符号链接：当可执行文件是 `target/debug/` 下指向 `deps/` 的链接时，
/// 目录会变成 `deps/`，导致 `modules/`、`locale/`、`config/` 全部找不到（后处理模块列表为空）。
/// 因此按以下顺序挑选候选目录：启动参数 argv[0] 的目录 → `current_exe()` 的目录 →
/// 从候选目录向上回溯的父目录；取第一个真正包含 `modules/` 或 `locale/` 的目录。
///
/// Returns the directory containing the executable, used to locate config files and module directories.
///
/// `current_exe()` resolves symlinks: when the executable is a link under `target/debug/` that
/// points into `deps/`, the directory becomes `deps/`, so `modules/`, `locale/` and `config/`
/// are all missed (the post-processing module list comes back empty). Candidates are therefore
/// tried in this order: the directory of argv[0] as launched → the directory of `current_exe()` →
/// parent directories of those; the first one that actually contains `modules/` or `locale/` wins.
pub fn exe_dir() -> PathBuf {
    use std::sync::OnceLock;
    static CACHE: OnceLock<PathBuf> = OnceLock::new();
    CACHE.get_or_init(compute_exe_dir).clone()
}

/// 计算 `exe_dir()` 的实际结果（见上方的候选目录规则）。
/// Compute the actual `exe_dir()` result (candidate rules described above).
fn compute_exe_dir() -> PathBuf {
    /// 把候选目录规范化为绝对路径；相对路径按当前工作目录展开。
    /// Normalize a candidate into an absolute path; relative paths resolve against the CWD.
    fn absolute(dir: &Path) -> PathBuf {
        let joined = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(dir))
                .unwrap_or_else(|_| dir.to_path_buf())
        };
        std::fs::canonicalize(&joined).unwrap_or(joined)
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1) 启动参数 argv[0]（不解析符号链接，反映用户实际调用的路径）
    // 1) argv[0] as launched (symlinks not resolved, reflects the path the user invoked)
    if let Some(arg0) = std::env::args_os().next() {
        let path = PathBuf::from(arg0);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            candidates.push(absolute(parent));
        }
    }

    // 2) current_exe() 的目录（可能已被解析到 deps/）
    // 2) The directory of current_exe() (may already be resolved into deps/)
    let current_exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(absolute));
    if let Some(dir) = current_exe_dir.clone()
        && !candidates.contains(&dir)
    {
        candidates.push(dir);
    }

    // 3) 优先选择包含 modules/ 或 locale/ 的候选目录
    // 3) Prefer a candidate that actually contains modules/ or locale/
    for marker in ["modules", "locale"] {
        for dir in &candidates {
            if dir.join(marker).is_dir() {
                return dir.clone();
            }
        }
    }

    // 4) cargo 的 deps/ 布局：从 current_exe() 目录向上回溯最多 3 层
    // 4) cargo's deps/ layout: walk up to 3 ancestors from the current_exe() directory
    if let Some(dir) = current_exe_dir {
        let mut cur: &Path = &dir;
        for _ in 0..3 {
            let Some(parent) = cur.parent() else { break };
            if parent.join("modules").is_dir() || parent.join("locale").is_dir() {
                return parent.to_path_buf();
            }
            cur = parent;
        }
    }

    // 5) 全部落空：退回第一个候选目录（保持旧行为）
    // 5) Nothing matched: fall back to the first candidate (preserves the old behaviour)
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

impl Default for Settings {
    fn default() -> Self {
        // 默认输出目录为可执行文件同目录下的 recordings 文件夹
        // Default output directory is the recordings folder next to the executable
        let output_dir = exe_dir().join("recordings").to_string_lossy().to_string();

        Self {
            output_dir,
            final_video_dir: String::new(),
            poll_interval_secs: 30,
            auto_record: true,
            api_proxy_url: None,
            cdn_proxy_url: None,
            sc_mirror_url: None,
            max_concurrent: 0,
            max_recording_duration_secs: 0,
            merge_format: default_merge_format(),
            preferred_resolution: 0,
            resolution_preference: default_resolution_preference(),
            max_tmp_dir_gb: default_max_tmp_dir_gb(),
            language: default_language(),
            server_port: default_server_port(),
            mouflon_sync_url: default_mouflon_sync_url(),
            mouflon_sync_token: None,
            setup_done: false,
            pp_concurrency: default_pp_concurrency(),
        }
    }
}

impl Settings {
    /// 最终视频所在根目录；`final_video_dir` 为空时回退到 `output_dir`。
    /// Root directory holding final videos; falls back to `output_dir` when `final_video_dir` is empty.
    pub fn videos_root(&self) -> PathBuf {
        let custom = self.final_video_dir.trim();
        if custom.is_empty() {
            PathBuf::from(&self.output_dir)
        } else {
            PathBuf::from(custom)
        }
    }

    /// 是否将最终视频与分片分开存放。
    /// Whether final videos are stored separately from the segment directories.
    pub fn has_separate_video_dir(&self) -> bool {
        self.videos_root() != PathBuf::from(&self.output_dir)
    }

    /// 计算录制会话目录对应的最终视频路径：`{videos_root}/{username}/{stem}.{fmt}`。
    /// 若会话目录直接位于输出目录下（旧布局），则不再额外插入用户名层级。
    ///
    /// Compute the final video path for a session directory: `{videos_root}/{username}/{stem}.{fmt}`.
    /// When the session dir sits directly under the output dir (legacy layout), no username level is added.
    pub fn video_path_for_session_with_format(&self, session_dir: &std::path::Path, fmt: &str) -> PathBuf {
        let stem = session_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let output_root = Path::new(self.output_dir.trim());
        let username = match session_dir.parent() {
            Some(parent) if parent != output_root => {
                parent.file_name().and_then(|n| n.to_str()).unwrap_or("")
            }
            _ => "",
        };
        let root = self.videos_root();
        let dir = if username.is_empty() {
            root
        } else {
            root.join(username)
        };
        dir.join(format!("{}.{}", stem, fmt))
    }

    /// 计算录制会话目录对应的最终视频路径（使用当前 `merge_format`）。
    /// Compute the final video path for a session directory (using the current `merge_format`).
    pub fn video_path_for_session(&self, session_dir: &std::path::Path) -> PathBuf {
        self.video_path_for_session_with_format(session_dir, &self.merge_format)
    }

    /// 计算最终视频对应的录制会话（.ts 分片）目录。
    /// Compute the recording session (.ts segment) directory that belongs to a final video.
    pub fn session_dir_for_video(&self, video_path: &std::path::Path) -> PathBuf {
        let stem = video_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let videos_root = self.videos_root();
        let username = match video_path.parent() {
            Some(parent) if parent != videos_root => {
                parent.file_name().and_then(|n| n.to_str()).unwrap_or("")
            }
            _ => "",
        };
        let output_root = PathBuf::from(self.output_dir.trim());
        let dir = if username.is_empty() {
            output_root
        } else {
            output_root.join(username)
        };
        dir.join(stem)
    }

    /// 计算最终视频对应的元数据文件路径：`.{stem}.json`，存放在**输出目录**中、
    /// 与分片目录同级的隐藏文件。最终视频目录只保存成片，不存放任何元数据。
    ///
    /// 输出目录与最终视频目录相同时，该路径与「视频同目录」的旧布局完全一致。
    ///
    /// Compute the metadata file path for a final video: `.{stem}.json`, a hidden file that
    /// lives in the **output directory**, as a sibling of the segment directory.
    /// The final video directory only ever holds merged videos.
    ///
    /// When the output dir and the final video dir are the same, this is identical to the
    /// legacy "next to the video" layout.
    pub fn meta_path_for_video(&self, video_path: &std::path::Path) -> Option<PathBuf> {
        let stem = video_path.file_stem()?.to_str()?;
        let session_dir = self.session_dir_for_video(video_path);
        Some(session_dir.with_file_name(format!(".{}.json", stem)))
    }

    /// 旧布局（元数据与视频同目录）下的元数据路径，用于升级时迁移遗留文件。
    /// Legacy (metadata next to the video) meta path, used to migrate files on upgrade.
    pub fn legacy_meta_path_for_video(video_path: &std::path::Path) -> Option<PathBuf> {
        let parent = video_path.parent()?;
        let stem = video_path.file_stem()?.to_str()?;
        Some(parent.join(format!(".{}.json", stem)))
    }

    /// 模块副产物（联系表图片等）应写入的目录：与元数据文件同目录，即输出目录。
    /// Directory where module side outputs (contact sheets etc.) should be written:
    /// the directory of the metadata file, i.e. the output directory.
    pub fn sidecar_dir_for_video(&self, video_path: &std::path::Path) -> Option<PathBuf> {
        self.meta_path_for_video(video_path)
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    /// 需要扫描最终视频的所有根目录（去重；最终视频目录优先，其次是遗留的输出目录）。
    /// All roots that may contain final videos (deduplicated; video dir first, then legacy output dir).
    pub fn video_scan_roots(&self) -> Vec<PathBuf> {
        let videos = self.videos_root();
        let output = PathBuf::from(&self.output_dir);
        if videos == output {
            vec![videos]
        } else {
            vec![videos, output]
        }
    }
}


/// 持久化到 config/ 目录下各 JSON 文件的全部应用数据 / All application data persisted to JSON files under the config/ directory
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppData {
    /// 用户配置 / User settings
    pub settings: Settings,
    /// 追踪的主播列表 / List of tracked streamers
    pub streamers: Vec<StreamerData>,
    /// Mouflon HLS 解密密钥存储（含密钥对和时间戳）/ Mouflon HLS decryption key store (keys + timestamps)
    #[serde(default)]
    pub mouflon_keys: MouflonKeysStore,
    /// 后处理流水线配置 / Post-processing pipeline configuration
    #[serde(default)]
    pub pipeline: PipelineConfig,
    /// 已执行过后处理的视频路径列表（目录文件，true/false 由对应 meta JSON 确认）
    /// List of video paths that have been post-processed (directory file; success/failure confirmed by reading the corresponding meta JSON)
    #[serde(default)]
    pub pp_results: Vec<String>,
}

/// 单个主播的持久化数据 / Persisted data for a single streamer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerData {
    /// 主播用户名（小写）/ Streamer username (lowercase)
    pub username: String,
    /// 是否开启自动录制 / Whether auto-record is enabled
    pub auto_record: bool,
    /// 添加时间（RFC 3339 格式）/ Time added (RFC 3339 format)
    pub added_at: String,
}

/// 应用运行时全局状态，通过 `Arc<AppState>` 在各模块间共享。
/// Global application runtime state, shared across modules via `Arc<AppState>`.
pub struct AppState {
    /// 持久化数据（受读写锁保护）/ Persisted data (protected by read-write lock)
    pub data: RwLock<AppData>,
    /// 配置目录路径（exe_dir/config/）/ Config directory path (exe_dir/config/)
    config_dir: PathBuf,
    /// 后处理任务状态表（文件路径 -> 任务状态）/ Post-processing task status map (file path -> status)
    pub pp_tasks: RwLock<HashMap<String, PpTaskStatus>>,
    /// 后处理取消标志（文件路径 -> 原子布尔）/ Post-processing cancel flags (file path -> atomic bool)
    pub pp_cancel_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
    /// 后处理并发闸门：限制同时运行的后处理流水线数量。
    /// 后处理包含分钟级的 Telegram 上传；串行执行会让合并/上传互相排队。
    /// 上限通过设置 `pp_concurrency` 调整（默认 3，在载入设置时生效）。
    /// Concurrency gate for post-processing: bounds how many pipeline runs execute
    /// simultaneously. Post-processing includes minutes-long Telegram uploads; a
    /// serial lock made merges and uploads queue behind each other. The limit is
    /// adjustable via the `pp_concurrency` setting (default 3, applied on load).
    pub pp_gate: TgGate,
    /// 启动合并锁，防止启动时的合并与正常录制并发 / Startup merge lock preventing concurrent startup merge and normal recording
    pub startup_lock: std::sync::Mutex<()>,    /// 通知监控器 poll_interval_secs 已变更的发送端（可选，启动后注入）
    /// Sender to notify the monitor that poll_interval_secs has changed (optional, injected after startup)
    pub poll_interval_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
    /// 通知 Mouflon 同步调度器立即触发同步的发送端（可选，启动后注入）
    /// Sender to notify the Mouflon sync scheduler to trigger an immediate sync (optional, injected after startup)
    pub mouflon_sync_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
    /// 手动发送到 Telegram 的任务状态表（视频路径 -> 状态）
    /// Manual "send to Telegram" task map (video path -> status)
    pub tg_tasks: RwLock<HashMap<String, TgSendStatus>>,
    /// 手动发送的取消标志（视频路径 -> 原子布尔）
    /// Manual-send cancel flags (video path -> atomic bool)
    pub tg_cancel_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
    /// 手动发送并发闸门：同一时刻最多运行 batch_size 个上传（发送对话框里可调）。
    /// Concurrency gate for manual sends: at most `batch_size` uploads run at once
    /// (adjustable from the send dialog).
    pub tg_gate: TgGate,
}

/// 手动发送的并发闸门：限制同时运行的上传数量。
/// Concurrency gate for manual sends: bounds the number of simultaneous uploads.
///
/// 限制值可以随时调整（发送对话框里的「同时上传数量」）；新值立即生效——
/// 正在运行的上传不会被中断，但新的上传会按新限制排队。
/// The limit can be changed at any time (the "concurrent uploads" field in the send
/// dialog); it applies immediately — in-flight uploads are never interrupted, but new
/// ones queue according to the new limit.
pub struct TgGate {
    /// (limit, active) — 允许的并发上限与当前运行数 / (limit, active)
    inner: std::sync::Mutex<(usize, usize)>,
    /// 有任务结束或限制变更时唤醒等待者 / Wakes waiters when a task finishes or the limit changes
    cv: std::sync::Condvar,
}

impl TgGate {
    /// 创建一个并发上限为 `limit` 的闸门 / Create a gate with the given concurrency limit
    pub fn new(limit: usize) -> Self {
        Self {
            inner: std::sync::Mutex::new((limit.max(1), 0)),
            cv: std::sync::Condvar::new(),
        }
    }

    /// 设置并发上限（至少为 1）。已在运行的上传不受影响。
    /// Set the concurrency limit (minimum 1). In-flight uploads are unaffected.
    pub fn set_limit(&self, limit: usize) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0 = limit.max(1);
        drop(g);
        self.cv.notify_all();
    }

    /// 当前并发上限 / Current concurrency limit
    pub fn limit(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
    }

    /// 当前运行中的上传数 / Number of uploads currently running
    pub fn active(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
    }

    /// 获取一个并发名额；排队期间被取消则返回 `None`。
    /// Acquire a concurrency slot; returns `None` if cancelled while queued.
    ///
    /// 等待以 1 秒为周期醒来检查取消标志，这样取消一个排队中的发送不需要
    /// 其他任务结束就能生效。
    /// The wait wakes every second to re-check the cancel flag, so cancelling a queued
    /// send takes effect without waiting for another task to finish.
    pub fn acquire(&self, cancel: &AtomicBool) -> Option<TgPermit<'_>> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            if guard.1 < guard.0 {
                guard.1 += 1;
                return Some(TgPermit { gate: self });
            }
            let (g2, _timeout) = self
                .cv
                .wait_timeout(guard, std::time::Duration::from_secs(1))
                .unwrap_or_else(|e| e.into_inner());
            guard = g2;
        }
    }
}

/// 并发名额的守卫：释放时把运行数减一并唤醒等待者。
/// Guard for a concurrency slot: dropping it decrements the running count and wakes waiters.
pub struct TgPermit<'a> {
    gate: &'a TgGate,
}

impl Drop for TgPermit<'_> {
    fn drop(&mut self) {
        let mut g = self.gate.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1 = g.1.saturating_sub(1);
        drop(g);
        self.gate.cv.notify_all();
    }
}

impl AppState {
    /// 返回配置目录路径（exe_dir/config/）。
    /// Returns the config directory path (exe_dir/config/).
    pub fn config_dir() -> PathBuf {
        exe_dir().join("config")
    }

    /// 从磁盘加载配置并初始化应用状态，确保输出目录存在。
    /// Load configuration from disk and initialize application state, ensuring the output directory exists.
    pub fn new() -> Result<Arc<Self>> {
        let config_dir = Self::config_dir();
        fs::create_dir_all(&config_dir)?;

        // 从拆分文件加载各部分数据 / Load each section from split files
        let load_json = |name: &str| -> Option<String> {
            fs::read_to_string(config_dir.join(name)).ok()
        };

        let settings: Settings = load_json("settings.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let streamers: Vec<StreamerData> = load_json("streamers.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mouflon_keys: MouflonKeysStore = load_json("mouflon_keys.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let pipeline: PipelineConfig = load_json("pipeline.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // pp_results.json 存储已执行过后处理的视频路径列表（Vec<String>）
        // 兼容旧格式（HashMap<String, bool>）：若解析为 Vec 失败，尝试解析为旧格式并提取 keys
        // pp_results.json stores a list of video paths that have been post-processed (Vec<String>)
        // Compatibility with old format (HashMap<String, bool>): if Vec parse fails, try old format and extract keys
        let pp_results: Vec<String> = load_json("pp_results.json")
            .and_then(|s| {
                serde_json::from_str::<Vec<String>>(&s).ok().or_else(|| {
                    serde_json::from_str::<HashMap<String, bool>>(&s)
                        .ok()
                        .map(|m| m.into_keys().collect())
                })
            })
            .unwrap_or_default();

        let pp_concurrency = settings.pp_concurrency.max(1);
        let data = AppData { settings, streamers, mouflon_keys, pipeline, pp_results };

        fs::create_dir_all(&data.settings.output_dir)?;
        fs::create_dir_all(data.settings.videos_root())?;

        Ok(Arc::new(Self {
            data: RwLock::new(data),
            config_dir,
            pp_tasks: RwLock::new(HashMap::new()),
            pp_cancel_flags: RwLock::new(HashMap::new()),
            pp_gate: TgGate::new(pp_concurrency),
            startup_lock: std::sync::Mutex::new(()),
            poll_interval_notify_tx: RwLock::new(None),
            mouflon_sync_notify_tx: RwLock::new(None),
            tg_tasks: RwLock::new(HashMap::new()),
            tg_cancel_flags: RwLock::new(HashMap::new()),
            tg_gate: TgGate::new(3),
        }))
    }

    /// 返回日志目录路径（可执行文件同目录下的 logs 文件夹）。
    /// Returns the log directory path (logs folder next to the executable).
    pub fn log_dir() -> PathBuf {
        exe_dir().join("logs")
    }

    /// 将当前 `AppData` 拆分序列化并分别写入各配置文件。
    /// Serialize the current `AppData` into split config files.
    pub fn save(&self) -> Result<()> {
        let data = self.data.read();
        let dir = &self.config_dir;
        fs::write(dir.join("settings.json"), serde_json::to_string_pretty(&data.settings)?)?;
        fs::write(dir.join("streamers.json"), serde_json::to_string_pretty(&data.streamers)?)?;
        fs::write(dir.join("mouflon_keys.json"), serde_json::to_string_pretty(&data.mouflon_keys)?)?;
        fs::write(dir.join("pipeline.json"), serde_json::to_string_pretty(&data.pipeline)?)?;
        fs::write(dir.join("pp_results.json"), serde_json::to_string_pretty(&data.pp_results)?)?;
        Ok(())
    }

    /// 获取当前设置的克隆副本。
    /// Get a cloned copy of the current settings.
    pub fn get_settings(&self) -> Settings {
        self.data.read().settings.clone()
    }

    /// 更新设置并保存到磁盘，同时确保新输出目录存在。
    /// 若 poll_interval_secs 发生变化，通知监控器立即以新间隔重新计时。
    /// 若 mouflon_sync_url 或 mouflon_sync_token 发生变化，通知同步调度器立即触发一次同步。
    ///
    /// Update settings and save to disk, also ensuring the new output directory exists.
    /// If poll_interval_secs changed, notify the monitor to restart its timer with the new interval.
    /// If mouflon_sync_url or mouflon_sync_token changed, notify the sync scheduler to trigger immediately.
    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        fs::create_dir_all(&settings.output_dir)?;
        fs::create_dir_all(settings.videos_root())?;
        let old = self.data.read().settings.clone();
        let poll_interval_changed = old.poll_interval_secs != settings.poll_interval_secs;
        let mouflon_sync_changed = old.mouflon_sync_url != settings.mouflon_sync_url
            || old.mouflon_sync_token != settings.mouflon_sync_token;
        self.data.write().settings = settings;
        self.save()?;
        if poll_interval_changed
            && let Some(tx) = self.poll_interval_notify_tx.read().as_ref() {
            let _ = tx.try_send(());
        }
        if mouflon_sync_changed
            && let Some(tx) = self.mouflon_sync_notify_tx.read().as_ref() {
            let _ = tx.try_send(());
        }
        Ok(())
    }

    /// 获取所有追踪主播的克隆列表。
    /// Get a cloned list of all tracked streamers.
    pub fn get_streamers(&self) -> Vec<StreamerData> {
        self.data.read().streamers.clone()
    }

    /// 添加新主播到追踪列表（若已存在则返回错误）。
    /// Add a new streamer to the tracking list (returns error if already exists).
    pub fn add_streamer(&self, username: &str) -> Result<()> {
        let mut data = self.data.write();
        if data.streamers.iter().any(|s| s.username == username) {
            return Err(AppError::Other(format!("模特 {} 已存在", username)));
        }
        let auto_record = data.settings.auto_record;
        data.streamers.push(StreamerData {
            username: username.to_string(),
            auto_record,
            added_at: chrono::Utc::now().to_rfc3339(),
        });
        drop(data);
        self.save()
    }

    /// 从追踪列表中移除主播并保存。
    /// Remove a streamer from the tracking list and save.
    pub fn remove_streamer(&self, username: &str) -> Result<()> {
        let mut data = self.data.write();
        data.streamers.retain(|s| s.username != username);
        drop(data);
        self.save()
    }

    /// 设置指定主播的自动录制开关并保存。
    /// Set the auto-record toggle for a specific streamer and save.
    pub fn set_auto_record(&self, username: &str, enabled: bool) -> Result<()> {
        let mut data = self.data.write();
        if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username) {
            s.auto_record = enabled;
        }
        drop(data);
        self.save()
    }

    /// 获取所有 Mouflon 解密密钥的克隆副本（仅 keys 部分，供录制/转发使用）。
    /// Get a cloned copy of all Mouflon decryption keys (keys map only, for recording/relay use).
    pub fn get_mouflon_keys(&self) -> HashMap<String, String> {
        self.data.read().mouflon_keys.keys.clone()
    }

    /// 获取完整的 Mouflon 密钥存储（含时间戳），供前端展示。
    /// Get the full Mouflon key store (including timestamps), for frontend display.
    pub fn get_mouflon_keys_store(&self) -> MouflonKeysStore {
        self.data.read().mouflon_keys.clone()
    }

    /// 添加或更新一个 Mouflon 密钥对，更新 manual_updated_at 并保存。
    /// Add or update a Mouflon key pair, update manual_updated_at, and save.
    pub fn add_mouflon_key(&self, pkey: &str, pdkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.keys.insert(pkey.to_string(), pdkey.to_string());
        data.mouflon_keys.manual_updated_at = Some(chrono::Utc::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 删除指定 pkey 的 Mouflon 密钥，更新 manual_updated_at 并保存。
    /// Remove the Mouflon key with the given pkey, update manual_updated_at, and save.
    pub fn remove_mouflon_key(&self, pkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.keys.remove(pkey);
        data.mouflon_keys.manual_updated_at = Some(chrono::Utc::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 从 Cloudflare Worker 同步 Mouflon 密钥。
    /// 比对 Worker 返回的 updated_at 与本地 auto_synced_at：
    ///   - 相同 → 跳过，返回 false（无需更新）
    ///   - 不同 → 覆盖 keys、更新 auto_synced_at，返回 true（已更新）
    ///
    /// Sync Mouflon keys from the Cloudflare Worker.
    /// Compares Worker's `updated_at` against local `auto_synced_at`:
    ///   - Equal   → skip, return false (no update needed)
    ///   - Different → overwrite keys, update auto_synced_at, return true (updated)
    pub async fn sync_mouflon_keys_from_worker(
        &self,
        worker_url: &str,
        auth_token: Option<&str>,
    ) -> Result<bool> {
        #[derive(Deserialize)]
        struct WorkerResponse {
            keys: HashMap<String, String>,
            updated_at: String,
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AppError::Other(e.to_string()))?;

        let mut req = client.get(worker_url);
        if let Some(token) = auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Other(format!("Worker 请求失败: {}", e)))?;

        if !resp.status().is_success() {
            return Err(AppError::Other(format!(
                "Worker 返回错误状态: {}",
                resp.status()
            )));
        }

        let body: WorkerResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("Worker 响应解析失败: {}", e)))?;

        // 比对 updated_at：解析为时间点后比较，避免格式差异导致误判
        // Compare updated_at by parsing to a time point, avoiding false mismatches due to format differences
        let worker_ts = chrono::DateTime::parse_from_rfc3339(&body.updated_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok();

        let same_timestamp = {
            let data = self.data.read();
            match (&worker_ts, &data.mouflon_keys.auto_synced_at) {
                (Some(wt), Some(local)) => {
                    chrono::DateTime::parse_from_rfc3339(local)
                        .map(|lt| lt.with_timezone(&chrono::Utc) == *wt)
                        .unwrap_or(false)
                }
                _ => false,
            }
        };

        if same_timestamp {
            // 时间戳相同，检查是否有本地缺失的 key / Same timestamp, check for locally missing keys
            let missing: Vec<(String, String)> = {
                let data = self.data.read();
                body.keys
                    .iter()
                    .filter(|(pkey, _)| !data.mouflon_keys.keys.contains_key(pkey.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            };
            if missing.is_empty() {
                return Ok(false);
            }
            // 补充缺失的 key / Insert missing keys
            let mut data = self.data.write();
            for (pkey, pdkey) in missing {
                data.mouflon_keys.keys.insert(pkey, pdkey);
            }
            drop(data);
            self.save()?;
            return Ok(true);
        }

        // 时间戳不同，插入本地不存在的键对，更新 auto_synced_at
        // Different timestamp: insert missing key pairs, update auto_synced_at
        {
            // 将 Worker 返回的时间戳规范化为 chrono RFC 3339 格式，与 manual_updated_at 保持一致
            // Normalize Worker timestamp to chrono RFC 3339 format, consistent with manual_updated_at
            let normalized_at = chrono::DateTime::parse_from_rfc3339(&body.updated_at)
                .map(|dt| dt.with_timezone(&chrono::Utc).to_rfc3339())
                .unwrap_or(body.updated_at);
            let mut data = self.data.write();
            for (pkey, pdkey) in body.keys {
                data.mouflon_keys.keys.entry(pkey).or_insert(pdkey);
            }
            data.mouflon_keys.auto_synced_at = Some(normalized_at);
        }
        self.save()?;
        Ok(true)
    }

    /// 获取当前流水线配置的克隆副本。
    /// Get a cloned copy of the current pipeline configuration.
    pub fn get_pipeline(&self) -> crate::postprocess::pipeline::PipelineConfig {
        self.data.read().pipeline.clone()
    }

    // ─── 手动发送到 Telegram 的任务表 / Manual "send to Telegram" task table ───

    /// 登记一个新的手动发送任务（若该路径已在运行则不覆盖）。
    /// 返回 false 表示该文件已经在发送中。
    ///
    /// Register a new manual-send task (does not overwrite a running one).
    /// Returns false when that file is already being sent.
    pub fn tg_task_start(
        &self,
        path: &str,
        chat_id: &str,
        delete_after_send: bool,
    ) -> bool {
        let mut tasks = self.tg_tasks.write();
        if tasks.get(path).is_some_and(|t| t.status == "running") {
            return false;
        }
        tasks.insert(
            path.to_string(),
            TgSendStatus {
                path: path.to_string(),
                status: "running".to_string(),
                pct: 0.0,
                status_text: String::new(),
                message: String::new(),
                started_at: chrono::Local::now().to_rfc3339(),
                delete_after_send,
                chat_id: chat_id.to_string(),
            },
        );
        // 重置（或创建）取消标志 / Reset (or create) the cancel flag
        self.tg_cancel_flags
            .write()
            .insert(path.to_string(), Arc::new(AtomicBool::new(false)));
        true
    }

    /// 取回（必要时创建）指定路径的手动发送取消标志。
    /// Get (creating if needed) the manual-send cancel flag for a path.
    pub fn tg_task_make_cancel_flag(&self, path: &str) -> Arc<AtomicBool> {
        let mut flags = self.tg_cancel_flags.write();
        if let Some(existing) = flags.get(path) {
            return Arc::clone(existing);
        }
        let flag = Arc::new(AtomicBool::new(false));
        flags.insert(path.to_string(), Arc::clone(&flag));
        flag
    }

    /// 请求中止指定路径的手动发送。
    /// Request cancellation of the manual send for a path.
    pub fn tg_task_cancel(&self, path: &str) {
        if let Some(flag) = self.tg_cancel_flags.read().get(path) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 更新手动发送进度 / Update manual-send progress
    pub fn tg_task_progress(&self, path: &str, pct: f64) {
        if let Some(t) = self.tg_tasks.write().get_mut(path) {
            t.pct = pct;
        }
    }

    /// 更新手动发送的状态文字（上传速度等），进度保持不变。
    /// Update the manual-send status text (upload speed etc.), leaving progress untouched.
    pub fn tg_task_status_text(&self, path: &str, status_text: &str) {
        if let Some(t) = self.tg_tasks.write().get_mut(path) {
            t.status_text = status_text.to_string();
        }
    }

    /// 结束手动发送任务（成功 / 失败 / 取消）。
    /// Finish a manual-send task (success / failure / cancelled).
    pub fn tg_task_finish(&self, path: &str, status: &str, message: &str) {
        if let Some(t) = self.tg_tasks.write().get_mut(path) {
            t.status = status.to_string();
            t.message = message.to_string();
            if status == "done" {
                t.pct = 100.0;
            }
        }
        self.tg_cancel_flags.write().remove(path);
    }

    /// 记录一次「还没开始就失败」的手动发送（例如视频不存在、没配置群组 ID）。
    /// 任务记录会被创建出来，这样界面上仍能看到失败原因，而不是什么都没发生。
    ///
    /// Record a manual send that failed before it could start (e.g. the video is missing or no
    /// chat id is configured). The task record is created anyway, so the UI can show why it
    /// failed instead of appearing to do nothing.
    pub fn tg_task_fail(&self, path: &str, message: &str, chat_id: &str, delete_after_send: bool) {
        let mut tasks = self.tg_tasks.write();
        let entry = tasks.entry(path.to_string()).or_insert_with(|| TgSendStatus {
            path: path.to_string(),
            status: "error".to_string(),
            pct: 0.0,
            status_text: String::new(),
            message: String::new(),
            started_at: chrono::Local::now().to_rfc3339(),
            delete_after_send,
            chat_id: chat_id.to_string(),
        });
        entry.status = "error".to_string();
        entry.message = message.to_string();
        self.tg_cancel_flags.write().remove(path);
    }

    /// 列出全部手动发送任务（运行中优先，其次按开始时间倒序）。
    /// List all manual-send tasks (running first, then newest first).
    pub fn get_tg_tasks(&self) -> Vec<TgSendStatus> {
        let mut list: Vec<TgSendStatus> = self.tg_tasks.read().values().cloned().collect();
        list.sort_by(|a, b| {
            let ra = a.status == "running";
            let rb = b.status == "running";
            rb.cmp(&ra).then_with(|| b.started_at.cmp(&a.started_at))
        });
        list
    }

    /// 删除任务记录（例如文件被手动删除后）。
    /// Drop a task record (e.g. after the file was deleted by hand).
    pub fn tg_task_remove(&self, path: &str) {
        self.tg_tasks.write().remove(path);
        self.tg_cancel_flags.write().remove(path);
    }
    /// 更新流水线配置并保存到磁盘。
    /// Update the pipeline configuration and save to disk.
    pub fn update_pipeline(&self, pipeline: crate::postprocess::pipeline::PipelineConfig) -> Result<()> {
        self.data.write().pipeline = pipeline;
        self.save()
    }

    /// 将指定文件路径的后处理任务加入等待队列。
    /// Enqueue a post-processing task for the given file path.
    pub fn pp_task_enqueue(&self, path: &str) {
        self.pp_tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total: 0,
                status: "waiting".to_string(),
                from_memory: true,
            },
        );
        // 确保取消标志存在（若已存在则不覆盖）/ Ensure cancel flag exists (don't overwrite if already present)
        self.pp_cancel_flags
            .write()
            .entry(path.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
    }

    /// 将指定文件路径的后处理任务标记为运行中。
    /// Mark the post-processing task for the given file path as running.
    pub fn pp_task_start(&self, path: &str, total: usize) {
        self.pp_tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total,
                status: "running".to_string(),
                from_memory: true,
            },
        );
    }

    /// 获取或创建指定文件路径的取消标志。
    /// Get or create the cancel flag for the given file path.
    pub fn pp_task_make_cancel_flag(&self, path: &str) -> Arc<AtomicBool> {
        let mut flags = self.pp_cancel_flags.write();
        if let Some(existing) = flags.get(path) {
            return Arc::clone(existing);
        }
        let flag = Arc::new(AtomicBool::new(false));
        flags.insert(path.to_string(), Arc::clone(&flag));
        flag
    }

    /// 设置指定文件路径的取消标志为 true，请求中止后处理。
    /// Set the cancel flag for the given file path to true, requesting post-processing abort.
    pub fn pp_task_cancel(&self, path: &str) {
        if let Some(flag) = self.pp_cancel_flags.read().get(path) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 清除指定文件路径的取消标志（任务完成后调用）。
    /// Clear the cancel flag for the given file path (called after task completes).
    pub fn pp_task_clear_cancel_flag(&self, path: &str) {
        self.pp_cancel_flags.write().remove(path);
    }

    /// 更新指定文件路径的后处理进度信息。
    /// Update the post-processing progress for the given file path.
    #[allow(clippy::too_many_arguments)]
    pub fn pp_task_progress(
        &self,
        path: &str,
        pct: f64,
        mod_done: u32,
        mod_total: u32,
        module_name: &str,
        done: usize,
        total: usize,
    ) {
        if let Some(t) = self.pp_tasks.write().get_mut(path) {
            t.pct = pct;
            t.mod_done = mod_done;
            t.mod_total = mod_total;
            t.module_name = module_name.to_string();
            t.done = done;
            t.total = total;
        }
    }

    /// 将后处理任务标记为完成或失败。成功/失败状态由对应 meta JSON 的 status 字段确认，
    /// 此处仅将路径记录到 pp_results 目录文件中（用于快速判断是否已执行过后处理）。
    ///
    /// Mark the post-processing task as done or failed. Success/failure is confirmed by the
    /// corresponding meta JSON's status field; here we only record the path in the pp_results
    /// directory file (for quick lookup of whether post-processing has been run).
    pub fn pp_task_finish(&self, path: &str, success: bool) {
        if let Some(t) = self.pp_tasks.write().get_mut(path) {
            t.status = if success { "done" } else { "error" }.to_string();
            t.pct = if success { 100.0 } else { t.pct };
        }
        // 将路径加入目录列表（去重）/ Add path to directory list (deduplicated)
        {
            let mut data = self.data.write();
            if !data.pp_results.contains(&path.to_string()) {
                data.pp_results.push(path.to_string());
            }
        }
        let _ = self.save();
    }

    /// 获取所有后处理任务状态的列表，合并内存中的运行时状态和持久化的历史结果。
    /// 历史结果通过读取对应 meta JSON 的 status 字段确认成功/失败。
    ///
    /// Get a list of all post-processing task statuses, merging in-memory runtime state with persisted historical results.
    /// Historical results are confirmed by reading the status field from the corresponding meta JSON.
    pub fn get_pp_tasks(&self) -> Vec<PpTaskStatus> {
        let mut tasks: HashMap<String, PpTaskStatus> = self.pp_tasks.read().clone();

        // 先取出设置与记录列表，避免在持有读锁时再次读取设置造成锁重入
        // Snapshot settings and the recorded list first, so settings can be read
        // without re-entering the data lock
        let (settings, pp_results) = {
            let data = self.data.read();
            (data.settings.clone(), data.pp_results.clone())
        };

        // 从 pp_results 目录文件补充历史任务，通过 meta 确认 success/failure
        // Supplement historical tasks from pp_results directory, confirming success/failure via meta
        for path in pp_results.iter() {
            if tasks.contains_key(path) {
                continue;
            }
            let video_path = std::path::Path::new(path);
            let success = crate::recording::meta::read_meta(&settings, video_path)
                .map(|m| m.status == "finish")
                .unwrap_or(false);
            tasks.insert(path.clone(), PpTaskStatus {
                path: path.clone(),
                pct: if success { 100.0 } else { 0.0 },
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total: 0,
                status: if success { "done" } else { "error" }.to_string(),
                from_memory: false,
            });
        }

        tasks.into_values().collect()
    }
}

/// 执行一次配置检查：验证所有追踪主播是否仍然存在，并检查孤立的后处理记录。
/// 若发现问题，通过 emitter 向前端发送 `startup-warnings` 事件。
///
/// Perform a single config check: verify all tracked streamers still exist,
/// and check for orphaned post-processing records.
/// If issues are found, emit a `startup-warnings` event to the frontend via the emitter.
pub async fn run_config_check(state: &Arc<AppState>, emitter: &Arc<dyn crate::core::emitter::Emitter>) {
    use crate::core::emitter::EmitterExt;
    use crate::core::error::AppError;

    let settings = state.get_settings();
    let streamers = state.get_streamers();

    let api = match crate::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    ) {
        Ok(a) => a,
        Err(_) => return,
    };

    // 每个主播最多重试 3 次，间隔 10 秒，确认不存在后才加入缺失列表
    // Retry up to 3 times per streamer with 10s delay; only add to missing list after confirmed
    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(10);

    let mut missing_streamers = Vec::new();
    for s in &streamers {
        let mut confirmed_missing = false;
        for attempt in 1..=MAX_ATTEMPTS {
            match api.get_stream_info(&s.username, false).await {
                Ok(_) => {
                    confirmed_missing = false;
                    break;
                }
                Err(AppError::UserNotFound(_)) => {
                    confirmed_missing = true;
                    break;
                }
                Err(_) => {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(RETRY_DELAY).await;
                    } else {
                        confirmed_missing = true;
                    }
                }
            }
        }
        if confirmed_missing {
            missing_streamers.push(s.username.clone());
        }
    }

    // 查找 pp_results 中对应文件已不存在的孤立记录
    // Find orphaned pp_results entries whose corresponding files no longer exist
    let missing_pp_results: Vec<String> = state
        .data
        .read()
        .pp_results
        .iter()
        .filter(|p| !std::path::Path::new(p.as_str()).exists())
        .cloned()
        .collect();

    if !missing_streamers.is_empty() || !missing_pp_results.is_empty() {
        emitter.emit(
            "startup-warnings",
            &serde_json::json!({
                "missing_streamers": missing_streamers,
                "missing_pp_results": missing_pp_results,
            }),
        );
    }
}

/// 启动配置检查调度器：立即执行一次检查，之后每天午夜执行一次。
/// Start the config check scheduler: run once immediately, then once every day at midnight.
pub async fn schedule_config_checks(state: Arc<AppState>, emitter: Arc<dyn crate::core::emitter::Emitter>) {
    run_config_check(&state, &emitter).await;

    loop {
        // 计算到下一个午夜的等待秒数 / Calculate seconds until next midnight
        let now = chrono::Local::now();
        let secs_until = {
            let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
            let midnight = tomorrow.and_hms_opt(0, 0, 0).unwrap();
            let midnight_local = midnight
                .and_local_timezone(chrono::Local)
                .single()
                .unwrap_or_else(|| now + chrono::Duration::hours(24));
            (midnight_local - now).num_seconds().max(0) as u64
        };
        tokio::time::sleep(tokio::time::Duration::from_secs(secs_until)).await;
        run_config_check(&state, &emitter).await;
    }
}

/// 启动 Mouflon Keys 自动同步调度器：启动时立即同步一次，之后每小时同步一次。
/// 若 Settings 中未配置 mouflon_sync_url，则静默跳过。
///
/// Start the Mouflon Keys auto-sync scheduler: sync once on startup, then every hour.
/// Silently skips if mouflon_sync_url is not configured in Settings.
pub async fn schedule_mouflon_sync(
    state: Arc<AppState>,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
    mut notify_rx: tokio::sync::mpsc::Receiver<()>,
) {
    use crate::core::emitter::EmitterExt;
    const INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(3600);

    loop {
        let settings = state.get_settings();
        if let Some(url) = settings.mouflon_sync_url.as_deref().filter(|u| !u.is_empty()) {
            let token = settings.mouflon_sync_token.as_deref();
            match state.sync_mouflon_keys_from_worker(url, token).await {
                Ok(true) => {
                    tracing::info!("Mouflon keys synced from {}", url);
                    emitter.emit(
                        "mouflon-keys-updated",
                        &state.get_mouflon_keys_store(),
                    );
                }
                Ok(false) => {
                    tracing::debug!("Mouflon keys up-to-date, skipped");
                }
                Err(e) => {
                    tracing::warn!("Mouflon keys sync failed: {}", e);
                }
            }
        }
        // 等待 1 小时，或收到立即同步通知
        // Wait 1 hour, or until an immediate sync notification arrives
        tokio::select! {
            _ = tokio::time::sleep(INTERVAL) => {}
            _ = notify_rx.recv() => {
                tracing::info!("Mouflon sync: settings changed, triggering immediate sync");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use std::path::{Path, PathBuf};

    /// 构造一个用于路径映射测试的设置 / Build settings for path-mapping tests
    fn settings(output_dir: &str, final_video_dir: &str) -> Settings {
        Settings {
            output_dir: output_dir.to_string(),
            final_video_dir: final_video_dir.to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn meta_lives_in_output_dir_when_dirs_are_split() {
        let s = settings("/rec", "/final");
        let video = Path::new("/final/model/model_20260101_120000.mp4");
        assert_eq!(
            s.meta_path_for_video(video),
            Some(PathBuf::from("/rec/model/.model_20260101_120000.json"))
        );
        assert_eq!(
            s.sidecar_dir_for_video(video),
            Some(PathBuf::from("/rec/model"))
        );
    }

    #[test]
    fn meta_matches_legacy_layout_when_dirs_are_the_same() {
        let s = settings("/rec", "");
        let video = Path::new("/rec/model/model_20260101_120000.mp4");
        let canonical = s.meta_path_for_video(video);
        assert_eq!(
            canonical,
            Settings::legacy_meta_path_for_video(video)
        );
        assert_eq!(
            canonical,
            Some(PathBuf::from("/rec/model/.model_20260101_120000.json"))
        );
    }

    #[test]
    fn flat_legacy_layout_has_no_username_level() {
        let s = settings("/rec", "");
        let video = Path::new("/rec/model_20260101_120000.mp4");
        assert_eq!(
            s.meta_path_for_video(video),
            Some(PathBuf::from("/rec/.model_20260101_120000.json"))
        );
    }

    #[test]
    fn video_path_round_trips_through_session_dir() {
        let s = settings("/rec", "/final");
        let session = Path::new("/rec/model/model_20260101_120000");
        let video = s.video_path_for_session(session);
        assert_eq!(video, PathBuf::from("/final/model/model_20260101_120000.mp4"));
        assert_eq!(s.session_dir_for_video(&video), session);
        assert_eq!(
            s.meta_path_for_video(&video),
            Some(PathBuf::from("/rec/model/.model_20260101_120000.json"))
        );
    }

    #[test]
    fn videos_root_falls_back_to_output_dir() {
        assert_eq!(settings("/rec", "   ").videos_root(), PathBuf::from("/rec"));
        assert_eq!(settings("/rec", "/final").videos_root(), PathBuf::from("/final"));
        assert!(!settings("/rec", "").has_separate_video_dir());
        assert!(settings("/rec", "/final").has_separate_video_dir());
    }

    /// 并发闸门的基本行为：限额、并发上限、取消退出。
    /// Basic gate behaviour: permits, concurrency limit, cancel-while-queued.
    #[test]
    fn tg_gate_admits_up_to_limit_concurrently() {
        use super::{TgGate, AtomicBool};

        let gate = TgGate::new(2);
        let no_cancel = AtomicBool::new(false);

        let p1 = gate.acquire(&no_cancel).unwrap();
        let p2 = gate.acquire(&no_cancel).unwrap();
        assert_eq!(gate.active(), 2);
        // 第 3 个应该排队（非阻塞验证：超时后仍没有名额）
        // A third would have to queue (non-blocking check: no permit appears)
        assert_eq!(gate.limit(), 2);

        drop(p2);
        assert_eq!(gate.active(), 1);
        drop(p1);
        assert_eq!(gate.active(), 0);

        // 限额收紧到 1 之后再取两个：第二个必须排队，取消后 acquire 返回 None
        // Tighten to 1: the second acquire must queue; cancelling makes it return None
        gate.set_limit(1);
        let q1 = gate.acquire(&no_cancel).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(gate.acquire(&cancel).is_none());
        drop(q1);
    }

    /// 限额变更立即生效：从 2 收紧到 1 后不能再同时拿到两个名额。
    /// Limit changes apply immediately: after tightening 2 → 1, two permits are impossible.
    #[test]
    fn tg_gate_limit_change_takes_effect_immediately() {
        use super::{TgGate, AtomicBool};

        let gate = TgGate::new(3);
        let no_cancel = AtomicBool::new(false);
        assert_eq!(gate.limit(), 3);
        gate.set_limit(0); // 至少 1 / minimum 1
        assert_eq!(gate.limit(), 1);
        gate.set_limit(5);
        let p1 = gate.acquire(&no_cancel).unwrap();
        let p2 = gate.acquire(&no_cancel).unwrap();
        assert_eq!(gate.active(), 2);
        drop(p1);
        drop(p2);
    }
}
