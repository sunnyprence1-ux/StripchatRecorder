//! 视频元数据文件管理 / Video Metadata File Management
//!
//! 每个录制对应一个隐藏的 JSON 元数据文件，文件名格式为 `.{stem}.json`。
//! 元数据文件在录制开始时创建，并随录制/合并/后处理各阶段持续更新 `status` 字段。
//!
//! 元数据文件始终保存在**输出目录**（与 `.ts` 分片目录同级），而不是最终视频目录：
//! 最终视频目录只存放合并完成的成片。当输出目录与最终视频目录相同时，
//! 路径与旧布局（视频同目录）完全一致。
//!
//! Each recording has a hidden JSON metadata file named `.{stem}.json`.
//! The file is created when recording starts and its `status` field is updated
//! throughout the recording / merging / post-processing lifecycle.
//!
//! Metadata files always live in the **output directory** (as a sibling of the `.ts`
//! segment directory), never in the final video directory, which only holds merged
//! videos. When both directories are the same, the paths match the legacy
//! "next to the video" layout.
//!
//! ## Status 状态流转 / Status lifecycle
//!
//! ```text
//! recording → merging_waiting → merging → pp_waiting → pp_running → finish
//!                                       ↘ finish (no pipeline)    ↘ pp_error
//! ```

use crate::config::settings::{AppState, Settings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 当前 meta 文件格式版本。
/// 每次对 `VideoMeta` 结构做不向后兼容的变更时递增此值，
/// 轮询扫描器会对版本不匹配的 meta 文件执行重建。
///
/// Current meta file format version.
/// Increment this whenever a breaking change is made to `VideoMeta`;
/// the periodic scanner will rebuild any meta file whose version doesn't match.
pub const META_VERSION: u32 = 2;

/// 从文件名 stem（格式：`{name}_{YYYYMMDD}_{HHmmss}`）中解析录制开始时间。
/// Parse the recording start time from a filename stem (format: `{name}_{YYYYMMDD}_{HHmmss}`).
pub fn parse_timestamp_from_stem(stem: &str) -> Option<String> {
    use chrono::TimeZone;
    let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
    if parts.len() < 2 {
        return None;
    }
    let time_part = parts[0];
    let date_part = parts[1];
    if date_part.len() == 8 && time_part.len() == 6 {
        let combined = format!("{}{}", date_part, time_part);
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&combined, "%Y%m%d%H%M%S") {
            let local = chrono::Local.from_local_datetime(&dt).single()?;
            return Some(local.to_rfc3339());
        }
    }
    None
}

/// 后处理模块的执行结果 / Execution result of a post-processing module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpModuleResult {
    /// 模块 ID / Module ID
    pub module_id: String,
    /// 是否成功 / Whether succeeded
    pub success: bool,
    /// 结果消息 / Result message
    pub message: String,
}

/// 视频元数据，持久化到 `.{stem}.json` 文件。
/// Video metadata persisted to `.{stem}.json`.
///
/// `status` 字段记录当前处理阶段，是前端展示的唯一依据：
/// - `"recording"`       — 正在录制
/// - `"merging_waiting"` — 等待合并（排队中）
/// - `"merging"`         — 正在合并 TS 分片
/// - `"pp_waiting"`      — 等待后处理（排队中）
/// - `"pp_running"`      — 后处理执行中
/// - `"pp_error"`        — 后处理失败
/// - `"finish"`          — 全部完成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMeta {
    /// meta 格式版本，用于检测结构变更后的重建需求。
    /// 缺失时反序列化为 0，触发重建。
    ///
    /// Meta format version, used to detect when a rebuild is needed after structural changes.
    /// Deserializes to 0 when absent, triggering a rebuild.
    #[serde(default)]
    pub meta_version: u32,

    /// 当前处理状态 / Current processing status
    pub status: String,

    /// 录制开始时间（RFC 3339 格式）/ Recording start time (RFC 3339 format)
    pub started_at: String,

    /// 文件大小（字节）/ File size (bytes)
    pub size_bytes: u64,

    /// 视频实际时长（秒，合并完成后由 ffprobe 填入）/ Actual video duration (seconds, filled by ffprobe after merge)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_duration_secs: Option<u64>,

    /// 各模块的后处理结果（后处理完成后填充）/ Per-module post-processing results (filled after completion)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_results: Option<Vec<PpModuleResult>>,

    /// 模块输出路径（模块 ID -> 输出文件路径）/ Module output paths (module ID -> output file path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_outputs: Option<std::collections::HashMap<String, String>>,
}

/// 根据视频文件路径计算对应的元数据文件路径（`.{stem}.json`，位于输出目录）。
/// Compute the metadata file path for a given video file path (`.{stem}.json`, in the output dir).
pub fn meta_path_for(settings: &Settings, video_path: &Path) -> Option<PathBuf> {
    settings.meta_path_for_video(video_path)
}

/// 读取指定元数据文件的原始内容。
/// Read the raw content of a given metadata file.
pub fn read_meta_file(meta_path: &Path) -> Option<VideoMeta> {
    let content = std::fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 读取视频文件对应的元数据，若文件不存在或解析失败则返回 `None`。
/// 优先读取输出目录中的标准位置，找不到时回退到旧布局（视频同目录）。
///
/// Read the metadata for a video file; returns `None` if the file doesn't exist or fails to parse.
/// Prefers the canonical location in the output dir, falling back to the legacy layout
/// (next to the video).
pub fn read_meta(settings: &Settings, video_path: &Path) -> Option<VideoMeta> {
    if let Some(meta_path) = settings.meta_path_for_video(video_path)
        && let Some(meta) = read_meta_file(&meta_path)
    {
        return Some(meta);
    }
    Settings::legacy_meta_path_for_video(video_path).and_then(|p| read_meta_file(&p))
}

/// 将元数据写入视频文件对应的 `.{stem}.json` 文件（输出目录）。
/// 写入前自动将 `meta_version` 设置为当前版本常量。
/// 若旧布局下视频同目录仍残留元数据文件，写入成功后将其删除（升级迁移）。
///
/// Write metadata to the `.{stem}.json` file for a video file (in the output dir).
/// Automatically sets `meta_version` to the current version constant before writing.
/// If a legacy metadata file still sits next to the video, it is removed after a
/// successful write (upgrade migration).
pub fn write_meta(settings: &Settings, video_path: &Path, meta: &VideoMeta) {
    let Some(meta_path) = settings.meta_path_for_video(video_path) else {
        return;
    };
    if let Some(parent) = meta_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("Failed to create meta dir {:?}: {}", parent, e);
        return;
    }
    let mut meta = meta.clone();
    meta.meta_version = META_VERSION;
    match serde_json::to_string_pretty(&meta) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&meta_path, json) {
                tracing::warn!("Failed to write meta {:?}: {}", meta_path, e);
            } else if let Some(legacy) = Settings::legacy_meta_path_for_video(video_path)
                && legacy != meta_path
                && legacy.exists()
            {
                // 旧布局残留：迁移后清理 / Legacy leftover: clean up after migrating
                let _ = std::fs::remove_file(&legacy);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize meta for {:?}: {}", video_path, e);
        }
    }
}

/// 删除视频文件对应的元数据文件（标准位置与旧布局残留都会删除）。
/// Delete the metadata file for a video file (both the canonical and legacy locations).
pub fn delete_meta(settings: &Settings, video_path: &Path) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = settings.meta_path_for_video(video_path) {
        candidates.push(p);
    }
    if let Some(p) = Settings::legacy_meta_path_for_video(video_path)
        && !candidates.contains(&p)
    {
        candidates.push(p);
    }
    for meta_path in candidates {
        if meta_path.exists() {
            let _ = std::fs::remove_file(&meta_path);
        }
    }
}

/// 一次「删除整条录制」操作的结果 / Result of one "delete the whole recording" operation
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    /// 实际删除的文件/目录路径 / Paths of the files and directories actually removed
    pub removed: Vec<String>,
    /// 释放的字节数（只统计成片、分片和副产物文件）
    /// Bytes freed (video, segments and side outputs only)
    pub freed_bytes: u64,
    /// 删除失败的项目（路径 + 原因）/ Items that could not be removed (path + reason)
    pub failed: Vec<String>,
}

impl PurgeReport {
    /// 是否真的删掉了东西 / Whether anything was actually removed
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.failed.is_empty()
    }
}

/// 删除一条录制在服务器上的**全部**数据：成片、`.ts` 分片目录、元数据、模块副产物，
/// 以及元数据里登记的模块输出文件。用于「上传成功后清空 VPS」。
///
/// 只删除确实存在的路径，任何一项失败都不会中断其余项的删除；
/// 空的模特子目录会被顺手清掉，非空目录则保留。
///
/// Delete **all** server-side data of one recording: the merged video, the `.ts` segment
/// directory, the metadata file, module side outputs, and any module output files recorded
/// in the metadata. Used for "wipe the VPS after a successful upload".
///
/// Only existing paths are touched, one failure never aborts the rest, and now-empty
/// per-model subdirectories are cleaned up (non-empty ones are kept).
pub fn purge_recording(settings: &Settings, video_path: &Path) -> PurgeReport {
    let mut report = PurgeReport::default();
    let stem = video_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    // 1) 先读元数据：里面登记的模块输出（联系表等）也要一起删
    // Read the metadata first: module outputs recorded in it (contact sheets etc.) must go too
    let meta = read_meta(settings, video_path);
    let module_outputs: Vec<PathBuf> = meta
        .as_ref()
        .and_then(|m| m.module_outputs.as_ref())
        .map(|m| m.values().map(PathBuf::from).collect())
        .unwrap_or_default();

    // 2) 删除单个文件（带重试：Windows 上 ffmpeg/预览可能短暂占用句柄）
    // Remove a single file (with retries: on Windows ffmpeg/preview may briefly hold a handle)
    let remove_file = |p: &Path, report: &mut PurgeReport| {
        if !p.is_file() {
            return;
        }
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let mut last_err = None;
        for _ in 0..10 {
            match std::fs::remove_file(p) {
                Ok(()) => {
                    report.removed.push(p.to_string_lossy().to_string());
                    report.freed_bytes += size;
                    return;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
        }
        report.failed.push(format!(
            "{}: {}",
            p.display(),
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ));
    };

    // 3) 成片本体 / The merged video itself
    remove_file(video_path, &mut report);

    // 4) 同一 stem 的其它扩展名成片（改过 merge_format 时可能残留）
    // Other extensions of the same stem (left behind after changing merge_format)
    if let Some(parent) = video_path.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let p = entry.path();
            if p == video_path || !p.is_file() {
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()) {
                remove_file(&p, &mut report);
            }
        }
    }

    // 5) 模块副产物：成片目录与输出目录（副产物目录）都要检查
    // Module side outputs: check both the video dir and the output dir (sidecar dir)
    if !stem.is_empty() {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(parent) = video_path.parent() {
            dirs.push(parent.to_path_buf());
        }
        if let Some(dir) = settings.sidecar_dir_for_video(video_path)
            && !dirs.contains(&dir)
        {
            dirs.push(dir);
        }
        for dir in &dirs {
            for ext in ["webp", "jpg", "jpeg", "png", "json"] {
                let sidecar = dir.join(format!("{}.{}", stem, ext));
                remove_file(&sidecar, &mut report);
            }
        }
    }

    // 6) 元数据里登记的模块输出（可能位于任意目录）
    // Module outputs recorded in the metadata (they may live anywhere)
    for out in &module_outputs {
        remove_file(out, &mut report);
    }

    // 7) 元数据文件本身（标准位置 + 旧布局残留）
    // The metadata file itself (canonical location + legacy leftovers)
    let mut meta_paths: Vec<PathBuf> = Vec::new();
    if let Some(p) = settings.meta_path_for_video(video_path) {
        meta_paths.push(p);
    }
    if let Some(p) = Settings::legacy_meta_path_for_video(video_path)
        && !meta_paths.contains(&p)
    {
        meta_paths.push(p);
    }
    for p in &meta_paths {
        remove_file(p, &mut report);
    }

    // 8) `.ts` 分片目录（整棵删掉：playlist.m3u8、分片、临时文件）
    // The `.ts` segment directory (removed wholesale: playlist.m3u8, segments, temp files)
    let session_dir = settings.session_dir_for_video(video_path);
    if session_dir.is_dir() {
        let size = dir_size(&session_dir);
        match std::fs::remove_dir_all(&session_dir) {
            Ok(()) => {
                report.removed.push(session_dir.to_string_lossy().to_string());
                report.freed_bytes += size;
            }
            Err(e) => report.failed.push(format!("{}: {}", session_dir.display(), e)),
        }
    }

    // 9) 顺手清掉变空的模特子目录（输出目录与最终视频目录各自检查，非空则保留）
    // Clean up now-empty per-model subdirectories (checked in both roots; non-empty ones stay)
    for root in settings.video_scan_roots() {
        if let Some(parent) = video_path.parent()
            && parent != root
            && parent.starts_with(&root)
            && parent.is_dir()
            && std::fs::read_dir(parent).map(|mut d| d.next().is_none()).unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
        if let Some(parent) = session_dir.parent()
            && parent != root
            && parent.starts_with(&root)
            && parent.is_dir()
            && std::fs::read_dir(parent).map(|mut d| d.next().is_none()).unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
    }

    if report.removed.is_empty() && report.failed.is_empty() {
        tracing::info!("purge_recording: nothing to delete for {:?}", video_path);
    } else {
        tracing::info!(
            "purge_recording: removed {} path(s), freed {} byte(s), {} failure(s) for {:?}",
            report.removed.len(),
            report.freed_bytes,
            report.failed.len(),
            video_path
        );
    }
    for f in &report.failed {
        tracing::warn!("purge_recording: failed to remove {}", f);
    }

    report
}

/// 递归统计目录占用的字节数 / Recursively total the bytes used by a directory
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = std::fs::metadata(&p) {
                total += m.len();
            }
        }
    }
    total
}

/// 将旧布局（视频同目录）中的元数据文件迁移到输出目录。
/// 标准位置已存在或旧文件不存在时不做任何操作。
///
/// Migrate a legacy metadata file (next to the video) into the output directory.
/// Does nothing when the canonical file already exists or the legacy one is missing.
fn migrate_legacy_meta(settings: &Settings, video_path: &Path) {
    let (Some(canonical), Some(legacy)) = (
        settings.meta_path_for_video(video_path),
        Settings::legacy_meta_path_for_video(video_path),
    ) else {
        return;
    };
    if canonical == legacy || canonical.exists() || !legacy.exists() {
        return;
    }
    if let Some(parent) = canonical.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("Failed to create meta dir {:?}: {}", parent, e);
        return;
    }
    if std::fs::rename(&legacy, &canonical).is_err() {
        if std::fs::copy(&legacy, &canonical).is_ok() {
            let _ = std::fs::remove_file(&legacy);
        } else {
            tracing::warn!(
                "Failed to migrate meta {:?} → {:?}",
                legacy,
                canonical
            );
            return;
        }
    }
    tracing::info!("Migrated meta {:?} → {:?}", legacy, canonical);
}

/// 仅更新 meta 文件的 `status` 字段，其余字段保持不变。
/// 若 meta 文件不存在则从视频文件信息重建后再写入。
///
/// Update only the `status` field of the meta file, leaving other fields unchanged.
/// If the meta file doesn't exist, rebuilds it from video file info before writing.
pub fn set_status(settings: &Settings, video_path: &Path, status: &str) {
    let mut meta = match read_meta(settings, video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                std::fs::metadata(video_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            });
            VideoMeta {
                meta_version: META_VERSION,
                status: status.to_string(),
                started_at,
                size_bytes,
                video_duration_secs: None,
                pp_results: None,
                module_outputs: None,
            }
        }
    };
    meta.status = status.to_string();
    write_meta(settings, video_path, &meta);
}

/// 后处理完成时更新 meta：写入最终状态、各模块结果和输出路径。
/// Update meta when post-processing completes: write final status, module results, and output paths.
pub fn set_pp_done(
    settings: &Settings,
    video_path: &Path,
    status: &str,
    results: Vec<PpModuleResult>,
    module_outputs: std::collections::HashMap<String, String>,
) {
    let mut meta = match read_meta(settings, video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                std::fs::metadata(video_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            });
            VideoMeta {
                meta_version: META_VERSION,
                status: status.to_string(),
                started_at,
                size_bytes,
                video_duration_secs: None,
                pp_results: None,
                module_outputs: None,
            }
        }
    };
    meta.status = status.to_string();
    meta.pp_results = Some(results);
    if !module_outputs.is_empty() {
        meta.module_outputs = Some(module_outputs);
    }
    write_meta(settings, video_path, &meta);
}

/// 若 meta 文件已存在则不覆盖，否则创建初始 meta（用于启动时遗留片段的保险创建）。
/// Does not overwrite if the meta file already exists; otherwise creates an initial meta
/// (used as a safety net for leftover segments on startup).
pub fn ensure_meta(settings: &Settings, video_path: &Path, started_at: &str) {
    if let Some(meta_path) = settings.meta_path_for_video(video_path)
        && meta_path.exists()
    {
        return;
    }
    let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
    let meta = VideoMeta {
        meta_version: META_VERSION,
        status: "merging_waiting".to_string(),
        started_at: started_at.to_string(),
        size_bytes,
        video_duration_secs: None,
        pp_results: None,
        module_outputs: None,
    };
    write_meta(settings, video_path, &meta);
}

/// 启动时扫描所有视频目录，为所有缺少 meta 文件的视频补写，并补全缺失的 `video_duration_secs`。
/// On startup, scan all video roots and write missing meta files for all videos,
/// also filling in missing `video_duration_secs` via ffprobe.
pub fn startup_ensure_meta_files(settings: &Settings) {
    ensure_meta_files(settings);
}

/// 扫描所有视频目录，为所有缺少或版本过旧的 meta 文件执行创建/重建，
/// 并补全缺失的 `video_duration_secs`；同时把旧布局（视频同目录）中的 meta 迁移到输出目录。
/// 跳过仍处于活跃状态（录制中/合并中/后处理中）的 meta，避免干扰正在进行的任务。
///
/// Scan all video roots and create/rebuild meta files that are missing or have an
/// outdated version, also filling in missing `video_duration_secs` via ffprobe;
/// legacy metadata files (next to the video) are migrated into the output directory.
/// Skips meta files in active states (recording/merging/post-processing) to avoid
/// interfering with ongoing tasks.
pub fn ensure_meta_files(settings: &Settings) {
    let mut count_created = 0usize;
    let mut count_updated = 0usize;
    for root in settings.video_scan_roots() {
        if !root.exists() {
            continue;
        }
        scan_and_ensure_meta(&root, settings, &mut count_created, &mut count_updated);
    }
    if count_created > 0 || count_updated > 0 {
        tracing::info!(
            "Meta scan: created {} new, rebuilt/updated {} existing meta files",
            count_created,
            count_updated
        );
    }
}

fn scan_and_ensure_meta(
    dir: &Path,
    settings: &Settings,
    count_created: &mut usize,
    count_updated: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            scan_and_ensure_meta(&path, settings, count_created, count_updated);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || ext != settings.merge_format {
                continue;
            }

            // 旧布局的 meta 先迁移到输出目录，后续读写都使用标准位置
            // Migrate legacy meta into the output dir first; all reads/writes then use the canonical path
            migrate_legacy_meta(settings, &path);

            match read_meta(settings, &path) {
                None => {
                    // meta 不存在：新建
                    // Meta missing: create it
                    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                        std::fs::metadata(&path)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .map(|t| {
                                let dt: chrono::DateTime<chrono::Local> = t.into();
                                dt.to_rfc3339()
                            })
                            .unwrap_or_default()
                    });
                    let video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                    let meta = VideoMeta {
                        meta_version: META_VERSION,
                        status: "finish".to_string(),
                        started_at,
                        size_bytes,
                        video_duration_secs,
                        pp_results: None,
                        module_outputs: None,
                    };
                    write_meta(settings, &path, &meta);
                    *count_created += 1;
                }
                Some(mut meta) => {
                    // 活跃状态的 meta 跳过，避免干扰正在进行的任务
                    // Skip active meta to avoid interfering with ongoing tasks
                    if matches!(
                        meta.status.as_str(),
                        "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                    ) {
                        continue;
                    }

                    let mut changed = false;

                    // 版本不匹配：重建 meta，保留可复用的字段
                    // Version mismatch: rebuild meta, preserving reusable fields
                    if meta.meta_version != META_VERSION {
                        tracing::info!(
                            "Meta version mismatch for {:?}: found {}, expected {} — rebuilding",
                            path,
                            meta.meta_version,
                            META_VERSION
                        );
                        // 重新从磁盘读取 size_bytes 和 video_duration_secs
                        // Re-read size_bytes and video_duration_secs from disk
                        meta.size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(meta.size_bytes);
                        if meta.video_duration_secs.is_none() {
                            meta.video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                        }
                        // 过渡态修正为 finish（版本升级时崩溃遗留）
                        // Correct transient status to finish (crash remnant during version upgrade)
                        if matches!(
                            meta.status.as_str(),
                            "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                        ) {
                            meta.status = "finish".to_string();
                        }
                        // meta_version 由 write_meta 自动设置为 META_VERSION
                        // meta_version is set to META_VERSION automatically by write_meta
                        write_meta(settings, &path, &meta);
                        *count_updated += 1;
                        continue;
                    }

                    // 版本匹配：仅补全缺失字段
                    // Version matches: only fill in missing fields
                    if meta.video_duration_secs.is_none() {
                        meta.video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                        changed = true;
                    }
                    if changed {
                        write_meta(settings, &path, &meta);
                        *count_updated += 1;
                    }
                }
            }
        }
    }
}

/// 由 meta 文件路径推导其所属的录制会话（.ts 分片）目录。
/// 标准位置（输出目录中与分片目录同级）和旧布局（视频同目录）都适用。
///
/// Derive the recording session (.ts segment) directory a meta file belongs to.
/// Works both for the canonical location (output dir, sibling of the segment dir)
/// and for the legacy layout (next to the video).
pub fn session_dir_for_meta(meta_path: &Path) -> Option<PathBuf> {
    let name = meta_path.file_name()?.to_str()?;
    let stem = name.strip_prefix('.')?.strip_suffix(".json")?;
    if stem.is_empty() {
        return None;
    }
    Some(meta_path.parent()?.join(stem))
}

/// 由 meta 文件路径推导其所属的最终视频路径。
/// Derive the final video path a meta file belongs to.
pub fn video_path_for_meta(settings: &Settings, meta_path: &Path) -> Option<PathBuf> {
    let session_dir = session_dir_for_meta(meta_path)?;
    Some(settings.video_path_for_session_with_format(&session_dir, &settings.merge_format))
}

/// meta 文件对应视频的存在性判断结果。
/// Presence verdict for the video belonging to a meta file.
#[derive(Debug, PartialEq, Eq)]
enum VideoPresence {
    /// 视频存在 / The video exists
    Present,
    /// 确认视频已被删除 / The video is confirmed gone
    Missing,
    /// 无法判断（例如最终视频目录已被改到别处）/ Cannot tell (e.g. the video dir was moved)
    Unknown,
}

/// 判断 meta 文件对应的视频是否仍然存在（不限定扩展名，兼容修改过合并格式的录制）。
/// 视频所在目录不存在时返回 `Unknown`：此时多半是最终视频目录被改到了别处，
/// 不能据此删除元数据，否则改一次目录就会丢掉全部历史元数据。
///
/// Whether the video belonging to a meta file still exists (any extension, so recordings
/// merged with a since-changed `merge_format` are still recognised). Returns `Unknown`
/// when the video's directory itself is gone: that usually means the final video directory
/// was moved elsewhere, and metadata must not be discarded on that basis.
fn meta_video_presence(settings: &Settings, meta_path: &Path) -> VideoPresence {
    let Some(video_path) = video_path_for_meta(settings, meta_path) else {
        return VideoPresence::Unknown;
    };
    if video_path.exists() {
        return VideoPresence::Present;
    }
    let Some(parent) = video_path.parent() else {
        return VideoPresence::Unknown;
    };
    if !parent.is_dir() {
        // 视频目录本身不存在：无法确认视频是否被删除
        // The video directory itself is missing: cannot confirm the video was deleted
        return VideoPresence::Unknown;
    }
    let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let same_stem = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let p = e.path();
            if !p.is_file() {
                return false;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !name.starts_with('.') && p.file_stem().and_then(|s| s.to_str()) == Some(stem)
        });
    if same_stem {
        VideoPresence::Present
    } else {
        VideoPresence::Missing
    }
}

/// 扫描输出目录（以及最终视频目录中旧布局遗留的 meta），
/// 删除所有对应视频文件已不存在的孤立 meta 文件（`.{stem}.json`）。
///
/// Scan the output directory (plus legacy meta files still sitting in the video dir)
/// and delete orphaned meta files (`.{stem}.json`) whose videos no longer exist.
///
/// 返回删除的文件数量。
/// Returns the number of deleted files.
pub fn cleanup_orphaned_meta_files(settings: &Settings) -> usize {
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(settings.output_dir.trim())];
    for root in settings.video_scan_roots() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let mut count = 0usize;
    for root in roots {
        if root.exists() {
            cleanup_orphaned_meta_recursive(&root, settings, &mut count);
        }
    }
    if count > 0 {
        tracing::info!("Meta cleanup: deleted {} orphaned meta file(s)", count);
    }
    count
}

fn cleanup_orphaned_meta_recursive(dir: &Path, settings: &Settings, count: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                cleanup_orphaned_meta_recursive(&path, settings, count);
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // meta 文件格式：以 '.' 开头，以 '.json' 结尾
        // Meta file format: starts with '.', ends with '.json'
        if !name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }

        // 仅在确认视频已被删除时清理；无法判断时保留元数据
        // Only clean up when the video is confirmed gone; keep metadata when unsure
        if meta_video_presence(settings, &path) != VideoPresence::Missing {
            continue;
        }

        // 跳过仍处于活跃状态的 meta（录制中/合并中/后处理中），
        // 此时视频文件尚未生成，不应视为孤立文件。
        // Skip meta files that are still in an active state (recording / merging / post-processing).
        // The video file doesn't exist yet at these stages, so they are not truly orphaned.
        let is_active = read_meta_file(&path)
            .map(|m| {
                matches!(
                    m.status.as_str(),
                    "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                )
            })
            .unwrap_or(false);

        if is_active {
            tracing::debug!("Meta cleanup: skipping active meta {:?}", path);
            continue;
        }

        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("Meta cleanup: failed to delete {:?}: {}", path, e);
        } else {
            tracing::info!("Meta cleanup: deleted orphaned meta {:?}", path);
            *count += 1;
        }
    }
}

/// 启动孤立 meta 文件清理调度器：立即执行一次，之后每小时执行一次。
/// Start the orphaned meta cleanup scheduler: run once immediately, then once every hour.
///
/// 每轮都会重新读取设置，因此输出目录 / 最终视频目录的变更无需重启即可生效。
/// Settings are re-read every round, so output/video dir changes apply without a restart.
pub async fn schedule_meta_cleanup(state: Arc<AppState>) {
    loop {
        let settings = state.get_settings();
        tokio::task::spawn_blocking(move || {
            cleanup_orphaned_meta_files(&settings);
        })
        .await
        .ok();

        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}

/// 启动 meta 版本检查轮询调度器：立即执行一次，之后每隔指定秒数执行一次。
/// 对版本不匹配或缺失的 meta 文件执行重建，跳过活跃状态的录制。
///
/// Start the meta version-check polling scheduler: run once immediately, then at the
/// specified interval. Rebuilds meta files with missing or mismatched versions,
/// skipping active recordings.
///
/// # 参数 / Parameters
/// - `state`: 应用状态（每轮重新读取设置）/ Application state (settings re-read every round)
/// - `interval_secs`: 轮询间隔（秒）/ Poll interval (seconds)
pub async fn schedule_meta_version_check(state: Arc<AppState>, interval_secs: u64) {
    loop {
        let settings = state.get_settings();
        tokio::task::spawn_blocking(move || {
            ensure_meta_files(&settings);
        })
        .await
        .ok();

        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在系统临时目录下创建唯一测试目录 / Create a unique test directory under the system temp dir
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "scr_meta_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_meta(status: &str) -> VideoMeta {
        VideoMeta {
            meta_version: META_VERSION,
            status: status.to_string(),
            started_at: "2026-01-01T12:00:00+08:00".to_string(),
            size_bytes: 0,
            video_duration_secs: None,
            pp_results: None,
            module_outputs: None,
        }
    }

    #[test]
    fn write_meta_goes_to_output_dir_not_video_dir() {
        let root = temp_root("split");
        let out = root.join("out");
        let fin = root.join("fin");
        let settings = Settings {
            output_dir: out.to_string_lossy().to_string(),
            final_video_dir: fin.to_string_lossy().to_string(),
            ..Settings::default()
        };
        let video = fin.join("model").join("model_20260101_120000.mp4");
        std::fs::create_dir_all(video.parent().unwrap()).unwrap();
        std::fs::write(&video, b"x").unwrap();

        write_meta(&settings, &video, &sample_meta("finish"));

        let meta = settings.meta_path_for_video(&video).unwrap();
        assert_eq!(meta, out.join("model").join(".model_20260101_120000.json"));
        assert!(meta.exists());
        // 最终视频目录中不得出现任何元数据文件
        // No metadata file may appear in the final video directory
        let stray: Vec<_> = std::fs::read_dir(video.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect();
        assert!(stray.is_empty(), "unexpected files in video dir: {:?}", stray);

        // 通过视频路径可以读回 / The meta can be read back through the video path
        assert_eq!(read_meta(&settings, &video).unwrap().status, "finish");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_meta_is_read_and_migrated() {
        let root = temp_root("legacy");
        let out = root.join("out");
        let fin = root.join("fin");
        let settings = Settings {
            output_dir: out.to_string_lossy().to_string(),
            final_video_dir: fin.to_string_lossy().to_string(),
            ..Settings::default()
        };
        let video = fin.join("model").join("model_20260101_120000.mp4");
        std::fs::create_dir_all(video.parent().unwrap()).unwrap();
        std::fs::write(&video, b"x").unwrap();

        // 旧布局：meta 与视频同目录 / Legacy layout: meta next to the video
        let legacy = Settings::legacy_meta_path_for_video(&video).unwrap();
        std::fs::write(&legacy, serde_json::to_string(&sample_meta("finish")).unwrap()).unwrap();

        // 旧文件仍可读取 / The legacy file is still readable
        assert_eq!(read_meta(&settings, &video).unwrap().status, "finish");

        // 迁移后旧文件消失、新位置存在 / After migration the old file is gone and the new one exists
        migrate_legacy_meta(&settings, &video);
        let canonical = settings.meta_path_for_video(&video).unwrap();
        assert!(canonical.exists());
        assert!(!legacy.exists());
        assert_eq!(read_meta_file(&canonical).unwrap().status, "finish");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn orphan_cleanup_removes_meta_only_when_video_is_really_gone() {
        let root = temp_root("orphan");
        let out = root.join("out");
        let fin = root.join("fin");
        let settings = Settings {
            output_dir: out.to_string_lossy().to_string(),
            final_video_dir: fin.to_string_lossy().to_string(),
            ..Settings::default()
        };

        // 1) 视频目录整体不存在（最终视频目录被改到别处）→ 保留元数据
        // 1) The whole video dir is missing (final video dir was moved) → keep the meta
        std::fs::create_dir_all(out.join("model")).unwrap();
        let moved_meta = out.join("model").join(".moved_20260101_120000.json");
        std::fs::write(
            &moved_meta,
            serde_json::to_string(&sample_meta("finish")).unwrap(),
        )
        .unwrap();
        cleanup_orphaned_meta_files(&settings);
        assert!(moved_meta.exists(), "meta must survive a moved video dir");

        // 2) 视频目录存在但视频已被删除 → 清理元数据
        // 2) The video dir exists but the video is gone → the meta is cleaned up
        std::fs::create_dir_all(fin.join("model")).unwrap();
        std::fs::write(fin.join("model").join("other_20260101_120000.mp4"), b"x").unwrap();
        let dead_meta = out.join("model").join(".dead_20260101_120000.json");
        std::fs::write(
            &dead_meta,
            serde_json::to_string(&sample_meta("finish")).unwrap(),
        )
        .unwrap();
        cleanup_orphaned_meta_files(&settings);
        assert!(!dead_meta.exists(), "orphaned meta should be deleted");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn meta_paths_map_back_to_video() {
        let settings = Settings {
            output_dir: "/rec".to_string(),
            final_video_dir: "/final".to_string(),
            ..Settings::default()
        };
        let meta = Path::new("/rec/model/.model_20260101_120000.json");
        assert_eq!(
            session_dir_for_meta(meta),
            Some(PathBuf::from("/rec/model/model_20260101_120000"))
        );
        assert_eq!(
            video_path_for_meta(&settings, meta),
            Some(PathBuf::from("/final/model/model_20260101_120000.mp4"))
        );
    }
}
