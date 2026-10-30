//! 后处理流水线命令 / Post-processing Pipeline Commands
//!
//! 提供模块发现、流水线配置读写、后处理任务触发/取消、进度查询和模块输出路径查询等功能。
//! Provides module discovery, pipeline config read/write,
//! post-processing task triggering/cancellation, progress queries, and module output path queries.

use crate::core::emitter::{Emitter, EmitterExt};
use crate::postprocess::pipeline::{discover_modules, run_pipeline, NodeResult, PipelineConfig};
use crate::config::settings::AppState;
use std::sync::Arc;

/// 公开的后处理入口（供录制完成后自动触发使用）。
/// 将任务加入等待队列后调用内部实现。
///
/// Public post-processing entry point (used for automatic triggering after recording completes).
/// Enqueues the task and then calls the inner implementation.
pub fn run_postprocess_for_path(
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    state.pp_task_enqueue(&path_str);
    emitter.emit(
        "postprocess-waiting",
        &serde_json::json!({ "path": path_str }),
    );

    // 更新元数据文件中的后处理状态 / Update post-processing status in metadata file
    crate::recording::meta::set_status(&state.get_settings(), video_path, "pp_waiting");

    run_postprocess_for_path_inner(video_path, pipeline, emitter, state);
}

/// 后处理流水线执行的核心实现（同步，在阻塞线程中调用）。
/// 获取并发名额 → 检查取消标志 → 执行流水线 → 上报进度和结果。
/// 名额上限由 `pp_concurrency` 设置控制（默认 3），可与上传并发数独立调整。
///
/// Core implementation of post-processing pipeline execution (synchronous, called in a blocking thread).
/// Acquires a concurrency slot (from `pp_gate`, limit = `pp_concurrency`, default 3) → checks cancel flag → runs pipeline → reports progress and results.
pub fn run_postprocess_for_path_inner(
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    // 先创建取消标志（供 gate 等待时检查，也供后续流程复用）
    // Create cancel flag first (checked while gate waits; reused by the pipeline below)
    let cancel_flag = state.pp_task_make_cancel_flag(&path_str);

    // 获取并发名额（取代原来的串行 pp_lock）：同一时刻最多 pp_concurrency 个后处理/上传
    // 任务运行，其余排队（等待时每秒检查取消标志，取消排队中的任务即时生效）。
    // Get a concurrency slot instead of the old serial `pp_lock`: at most
    // `pp_concurrency` post-processing/upload tasks run at once, others queue.
    // While waiting, the cancel flag is checked every second, so cancelling a queued
    // task takes effect immediately.
    let gate = &state.pp_gate;
    let _permit = match gate.acquire(&cancel_flag) {
        Some(p) => p,
        None => {
            state.pp_task_finish(&path_str, false);
            emitter.emit(
                "postprocess-done",
                &serde_json::json!({ "path": path_str, "results": [{
                    "nodeId": "",
                    "moduleId": "pipeline",
                    "success": false,
                    "message": "cancelled while waiting for a slot",
                    "output": null,
                }]}),
            );
            return;
        }
    };

    // 检查是否在等待名额期间已被取消 / Check if cancelled while waiting for the slot
    let already_cancelled = cancel_flag.load(std::sync::atomic::Ordering::Relaxed);
    if already_cancelled {
        state.pp_task_clear_cancel_flag(&path_str);
        state.pp_tasks.write().remove(&path_str);
        return;
    }

    let modules = discover_modules();

    // 从 meta 读取上次的后处理结果，用于跳过已成功的模块（重新后处理时复用）
    // Read previous pp_results from meta to skip already-succeeded modules on re-run
    let settings = state.get_settings();
    let meta_snapshot = crate::recording::meta::read_meta(&settings, video_path);
    tracing::info!(
        "pp re-run: video={:?}, meta_status={:?}, meta_pp_results_count={}",
        video_path,
        meta_snapshot.as_ref().map(|m| &m.status),
        meta_snapshot.as_ref().and_then(|m| m.pp_results.as_ref()).map(|r| r.len()).unwrap_or(0),
    );
    let prev_results: Vec<crate::recording::meta::PpModuleResult> = meta_snapshot
        .and_then(|m| m.pp_results)
        .unwrap_or_default();

    tracing::info!(
        "pp re-run: prev_results={:?}",
        prev_results.iter().map(|r| format!("{}={}", r.module_id, r.success)).collect::<Vec<_>>()
    );

    // 构建实际需要执行的节点列表：跳过上次已成功的模块
    // Build the list of nodes to actually run: skip modules that succeeded last time
    let effective_pipeline = {
        let mut p = pipeline.clone();
        p.nodes.retain(|n| {
            if !n.enabled {
                return true; // 保留禁用节点（run_pipeline 会自动跳过）/ Keep disabled nodes (run_pipeline skips them)
            }
            let skip = prev_results.iter().any(|r| r.module_id == n.module_id && r.success);
            tracing::info!("pp re-run: node module_id={} enabled={} skip={}", n.module_id, n.enabled, skip);
            !skip
        });
        p
    };

    // 预检：确认所有启用节点的模块都存在，缺失则直接报错
    // Pre-check: verify all enabled nodes have their modules available, fail fast if not
    let missing: Vec<&str> = effective_pipeline
        .nodes
        .iter()
        .filter(|n| n.enabled)
        .filter(|n| !modules.iter().any(|m| m.id == n.module_id))
        .map(|n| n.module_id.as_str())
        .collect();

    if !missing.is_empty() {
        let msg = format!(
            "后处理模块缺失：{}，请检查 modules/ 目录",
            missing.join(", ")
        );
        state.pp_task_finish(&path_str, false);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({ "path": path_str, "results": [{
                "nodeId": "",
                "moduleId": missing[0],
                "success": false,
                "message": msg,
                "output": null
            }] }),
        );
        return;
    }

    let total = effective_pipeline.nodes.iter().filter(|n| n.enabled).count();

    state.pp_task_start(&path_str, total);
    emitter.emit(
        "postprocess-started",
        &serde_json::json!({ "path": path_str }),
    );

    // 更新元数据文件：标记为运行中 / Update metadata file: mark as running
    crate::recording::meta::set_status(&settings, video_path, "pp_running");

    let new_results: Vec<NodeResult> = run_pipeline(
        video_path,
        &effective_pipeline,
        &modules,
        Some(cancel_flag),
        &settings,
        // 进度回调：更新状态并向前端发送进度事件
        // Progress callback: update state and emit progress event to frontend
        |node_done: usize, node_total: usize, mod_done: u32, mod_total: u32, module_name: &str, status_text: &str| {
            let pct_raw = if node_total == 0 {
                100.0
            } else if mod_total > 0 {
                let nt = node_total as f64;
                let nd = node_done as f64;
                let node_pct = (nd * 100.0) / nt;
                let slice = 100.0 / nt;
                let inner = ((mod_done as f64) * slice) / (mod_total as f64);
                (node_pct + inner).min(100.0)
            } else {
                ((node_done as f64) * 100.0) / (node_total as f64)
            };
            let pct = (pct_raw * 100.0).round() / 100.0;

            let display_name = if status_text.is_empty() {
                module_name.to_string()
            } else {
                format!("{} · {}", module_name, status_text)
            };

            state.pp_task_progress(
                &path_str,
                pct,
                mod_done,
                mod_total,
                &display_name,
                node_done,
                node_total,
            );

            emitter.emit(
                "postprocess-progress",
                &serde_json::json!({
                    "path": path_str,
                    "done": node_done,
                    "total": node_total,
                    "pct": pct,
                    "modDone": mod_done,
                    "modTotal": mod_total,
                    "moduleName": display_name,
                }),
            );
        },
        // 日志回调：将模块的 stdout/stderr 输出转发给前端
        // Log callback: forward module stdout/stderr output to the frontend
        |module_id, stream, line| {
            emitter.emit(
                "postprocess-log",
                &serde_json::json!({
                    "path": path_str,
                    "moduleId": module_id,
                    "stream": stream,
                    "line": line,
                }),
            );
        },
        // 删除回调：模块（如 notify_telegram）在上传成功后请求清空本地数据。
        // 删除整条录制的全部数据：成片、.ts 分片目录、元数据、模块副产物。
        //
        // Delete callback: a module (e.g. notify_telegram) asks to wipe the local data after
        // a successful upload. Removes everything for the recording: video, .ts segments,
        // metadata and module side outputs.
        |deleted_path| {
            let report = crate::recording::meta::purge_recording(&settings, deleted_path);
            let deleted_str = deleted_path.to_string_lossy().to_string();
            {
                let mut data = state.data.write();
                data.pp_results.retain(|p| p != &deleted_str);
            }
            let _ = state.save();
            state.pp_tasks.write().remove(&deleted_str);
            emitter.emit(
                "recording-purged",
                &serde_json::json!({
                    "path": deleted_str,
                    "removed": report.removed,
                    "freedBytes": report.freed_bytes,
                    "failed": report.failed,
                    "reason": "uploaded",
                }),
            );
        },
    );

    state.pp_task_clear_cancel_flag(&path_str);

    // 合并本次结果与上次已成功的结果，按原始 pipeline 节点顺序排列
    // Merge new results with previously succeeded results, ordered by original pipeline node order
    let results: Vec<NodeResult> = {
        let mut merged: Vec<NodeResult> = Vec::new();
        for node in pipeline.nodes.iter().filter(|n| n.enabled) {
            if let Some(r) = new_results.iter().find(|r| r.module_id == node.module_id) {
                merged.push(r.clone());
            } else if let Some(prev) = prev_results.iter().find(|r| r.module_id == node.module_id && r.success) {
                merged.push(NodeResult {
                    node_id: node.node_id.clone(),
                    module_id: prev.module_id.clone(),
                    success: true,
                    message: prev.message.clone(),
                    output: None,
                    delete_input: false,
                });
            }
        }
        merged
    };

    let all_ok = results.iter().all(|r| r.success);

    if !video_path.exists() {
        crate::recording::meta::delete_meta(&settings, video_path);
        {
            let mut data = state.data.write();
            data.pp_results.retain(|p| p != &path_str);
        }
        let _ = state.save();
        state.pp_tasks.write().remove(&path_str);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({ "path": path_str, "results": results }),
        );
        return;
    }

    state.pp_task_finish(&path_str, all_ok);

    {
        let final_status = if all_ok { "finish" } else { "pp_error" };
        let pp_module_results: Vec<crate::recording::meta::PpModuleResult> = results
            .iter()
            .map(|r| crate::recording::meta::PpModuleResult {
                module_id: r.module_id.clone(),
                success: r.success,
                message: r.message.clone(),
            })
            .collect();
        let mut module_outputs = std::collections::HashMap::new();
        if let Some(prev_meta) = crate::recording::meta::read_meta(&settings, video_path)
            && let Some(prev_outputs) = prev_meta.module_outputs {
            module_outputs.extend(prev_outputs);
        }
        for r in &results {
            if r.success
                && let Some(ref out_path) = r.output
                && out_path != video_path
            {
                module_outputs.insert(
                    r.module_id.clone(),
                    out_path.to_string_lossy().to_string(),
                );
            }
        }
        crate::recording::meta::set_pp_done(
            &settings,
            video_path,
            final_status,
            pp_module_results,
            module_outputs,
        );
    }

    emitter.emit(
        "postprocess-done",
        &serde_json::json!({ "path": path_str, "results": results }),
    );
}
