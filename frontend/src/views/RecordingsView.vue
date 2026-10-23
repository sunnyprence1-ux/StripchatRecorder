<!--
    录制文件管理页面 / Recording File Management View

    展示所有录制文件，按主播分组，支持：
    - 实时录制时长计时和录制速度显示
    - 磁盘空间监控
    - 文件合并进度跟踪
    - 后处理流水线触发和进度显示
    - Contact Sheet 预览图查看（带缩放/平移）
    - 单文件和批量删除
    - 多列排序和分组折叠

    Displays all recording files grouped by streamer, supporting:
    - Real-time recording duration timer and recording speed display
    - Disk space monitoring
    - File merge progress tracking
    - Post-processing pipeline triggering and progress display
    - Contact Sheet preview image viewing (with zoom/pan)
    - Single and batch file deletion
    - Multi-column sorting and group collapsing
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, computed, ref, watchEffect } from "vue";
	import { call, on } from "@/lib/api";
	import { useNotify } from "../composables/useNotify";
	import { usePostprocessStore } from "@/stores/postprocess";
	import { useRecordings } from "@/composables/useRecordings";
	import { usePostprocess, makePpProgress } from "@/composables/usePostprocess";
	import { useImagePreview } from "@/composables/useImagePreview";
	import { useTelegramSend } from "@/composables/useTelegramSend";
	import SendToTelegramDialog from "@/components/SendToTelegramDialog.vue";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Loader2, Image, Send } from "@lucide/vue";
	import { Progress } from "@/components/ui/progress";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow,
	} from "@/components/ui/table";
	import { formatSize, formatDuration } from "@/utils/format";
	import { useI18n } from "vue-i18n";

	const { toast, confirm } = useNotify();
	const { t } = useI18n();
	const ppStore = usePostprocessStore();
	/** 事件取消订阅函数列表 / Event unsubscribe function list */
	const unlisteners: (() => void)[] = [];
	/** 本地已发起删除的文件路径集合（用于过滤 recording-deleted 事件通知）/ Locally deleted paths (to filter recording-deleted notifications) */
	const localDeletedPaths = new Set<string>();
	/**
	 * 删除时正在后处理的文件路径集合。
	 * 与 localDeletedPaths 不同，此集合不在 recording-deleted 时清除，
	 * 而是等到 postprocess-done 事件处理完后才清除，确保能正确抑制后处理失败 toast。
	 *
	 * Paths that were being post-processed when deleted.
	 * Unlike localDeletedPaths, this set is NOT cleared on recording-deleted;
	 * it is cleared after postprocess-done is handled, so the failure toast is correctly suppressed.
	 */
	const ppCancelledByDelete = new Set<string>();

	/** 磁盘空间信息 / Disk space information */
	interface DiskSpace {
		total_bytes: number;
		available_bytes: number;
		used_bytes: number;
	}
	const diskSpace = ref<DiskSpace | null>(null);

	/**
	 * 从后端刷新磁盘空间信息。
	 * Refresh disk space information from the backend.
	 */
	async function refreshDiskSpace() {
		try {
			diskSpace.value = await call<DiskSpace>("get_disk_space");
		} catch {}
	}

	/** 各文件的实时录制速度（字节/秒）/ Real-time recording speed per file (bytes/second) */
	const recordingSpeed = ref<Record<string, number>>({});

	/** 合并进度（video_path -> {out_bytes, total_bytes}）/ Merge progress per video path */
	const mergeProgress = ref<Record<string, { out_bytes: number; total_bytes: number }>>({});

	const rec = useRecordings();
	const tg = useTelegramSend();

	/** 待发送到 Telegram 的文件路径（null = 对话框关闭）/ Paths queued for a Telegram send (null = dialog closed) */
	const tgDialogPaths = ref<string[] | null>(null);

	/**
	 * 打开「发送到 Telegram」对话框。
	 * Open the "send to Telegram" dialog.
	 *
	 * @param paths - 要发送的视频路径 / Video paths to send
	 */
	function openTelegramSend(paths: string[]) {
		if (paths.length === 0) return;
		tgDialogPaths.value = paths;
	}

	/** 已选中且可发送到 Telegram 的文件路径 / Selected files eligible for a Telegram send */
	const tgSelectablePaths = computed(() =>
		[...selected.value].filter(
			(p) =>
				!files.value.find((f) => f.path === p)?.is_recording &&
				!files.value.find((f) => f.path === p)?.status?.startsWith("merging") &&
				!tg.isSending(p),
		),
	);

	/**
	 * 把已选中的文件加入发送队列。
	 * Queue the selected files for sending.
	 */
	function sendSelectedToTelegram() {
		const paths = tgSelectablePaths.value;
		if (paths.length === 0) return;
		selected.value.clear();
		openTelegramSend(paths);
	}
	const {
		files,
		loading,
		elapsed,
		selected,
		selectedCount,
		collapsedGroups,
		groups,
		load,
		startTick,
		stopTick,
		scheduleDirRefresh,
		cleanup: recCleanup,
		toggleSort,
		sortIcon,
		toggleGroup,
		getFileChecked,
		setFileChecked,
		getGroupChecked,
		setGroupChecked,
		getAllChecked,
		setAllChecked,
	} = rec;

	/**
	 * 移动端排序条的可排序列。
	 * 移动端隐藏表头，排序入口改为列表上方的一排按钮，保证排序功能不丢失。
	 *
	 * Sortable columns for the mobile sort bar.
	 * The table header is hidden on mobile, so sorting moves to a button row above the list
	 * to keep the functionality available.
	 */
	const sortableColumns: {
		key: "size_bytes" | "started_at" | "video_duration_secs";
		labelKey: string;
	}[] = [
		{ key: "size_bytes", labelKey: "recordings.table.size" },
		{ key: "started_at", labelKey: "recordings.table.startTime" },
		{ key: "video_duration_secs", labelKey: "recordings.table.videoDuration" },
	];

	const pp = usePostprocess();
	const {
		ppStatus,
		ppProgress,
		moduleOutputs,
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile: ppRemoveFile,
	} = pp;

	/**
	 * 从当前 files.value 列表中同步模块输出路径到 moduleOutputs。
	 * 仅补充缺失的条目，不覆盖已有的（如推断值或实时更新值）。
	 * 用于 load() 之后确保 contact_sheet 等预览图按钮能正确显示。
	 *
	 * Sync module output paths from the current files.value list into moduleOutputs.
	 * Only fills in missing entries; does not overwrite existing ones (e.g. inferred or live-updated values).
	 * Called after load() to ensure preview buttons (e.g. contact_sheet) are correctly shown.
	 */
	function syncModuleOutputsFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				// meta 是持久化真相来源，直接覆盖写入（与 onMounted 初始化行为一致）
				// Meta is the persistent source of truth; overwrite directly (consistent with onMounted init)
				moduleOutputs.value[f.path] = {
					...moduleOutputs.value[f.path],
					...f.module_outputs,
				};
			}
		}
	}

	const preview = useImagePreview();
	const {
		previewOpen,
		previewUrl,
		previewTitle,
		previewScale,
		previewTranslate,
		previewViewportRef,
		previewImageRef,
		isDragging,
		viewportSize,
		resetPreviewTransform,
		onPreviewImageLoad,
		onPreviewWheel,
		onPreviewMousedown,
		onDocMousemove,
		onDocMouseup,
		openPreview,
	} = preview;

	/**
	 * 用系统默认程序打开录制文件。
	 * Open a recording file with the system default application.
	 */
	async function openFile(path: string) {
		await call("open_recording", { path });
	}

	/**
	 * 打开模块输出文件（使用预览弹窗）。
	 * Open module output file (preview dialog).
	 */
	async function openModuleOutput(filePath: string, moduleId: string) {
		const outputPath = moduleOutputs.value[filePath]?.[moduleId];
		if (!outputPath) return;
		openPreview(
			`/api/files?path=${encodeURIComponent(outputPath)}`,
			outputPath.split(/[\\/]/).pop() ?? "预览图",
		);
	}

	/**
	 * 删除单个录制文件（需要用户确认）。
	 * Delete a single recording file (requires user confirmation).
	 */
	async function deleteFile(f: {
		name: string;
		path: string;
		is_recording: boolean;
	}) {
		const ok = await confirm({
			title: t("recordings.delete.title"),
			message: t("recordings.delete.message", { name: f.name }),
			confirmText: t("recordings.delete.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			if (ppStatus.value[f.path] === "running") {
				ppCancelledByDelete.add(f.path);
				await call("cancel_postprocess", { path: f.path }).catch(() => {});
			}
			localDeletedPaths.add(f.path);
			await call("delete_recording", { path: f.path });
			files.value = files.value.filter((r) => r.path !== f.path);
			delete elapsed.value[f.path];
			ppRemoveFile(f.path);
			selected.value.delete(f.path);
			toast(t("recordings.delete.done", { name: f.name }), "success");
		} catch (e) {
			localDeletedPaths.delete(f.path);
			toast(String(e), "error");
		}
	}

	/**
	 * 批量删除已选中的文件（需要用户确认）。
	 * Batch delete selected files (requires user confirmation).
	 */
	async function deleteSelected() {
		const paths = [...selected.value];
		const count = paths.length;
		const ok = await confirm({
			title: t("recordings.delete.batchTitle"),
			message: t("recordings.delete.batchMessage", { count }),
			confirmText: t("recordings.delete.confirm"),
			danger: true,
		});
		if (!ok) return;
		await Promise.all(
			paths
				.filter((p) => ppStatus.value[p] === "running")
				.map((p) => {
					ppCancelledByDelete.add(p);
					return call("cancel_postprocess", { path: p }).catch(() => {});
				}),
		);
		let failed = 0;
		for (const path of paths) {
			try {
				localDeletedPaths.add(path);
				await call("delete_recording", { path });
				files.value = files.value.filter((r) => r.path !== path);
				delete elapsed.value[path];
				ppRemoveFile(path);
				selected.value.delete(path);
			} catch {
				localDeletedPaths.delete(path);
				failed++;
			}
		}
		if (failed > 0)
			toast(t("recordings.delete.batchFailed", { count: failed }), "error");
		else toast(t("recordings.delete.batchDone", { count }), "success");
	}

	/**
	 * 对所有已选中且符合条件的文件批量触发后处理。
	 * 按录制开始时间排序，确保处理顺序一致。
	 *
	 * Trigger post-processing for all selected eligible files in batch.
	 * Sorted by recording start time to ensure consistent processing order.
	 */
	async function postProcessSelected() {
		const paths = [...selected.value].filter(
			(p) =>
				ppStatus.value[p] !== "running" &&
				ppStatus.value[p] !== "waiting" &&
				!files.value.find((f) => f.path === p)?.is_recording &&
				!files.value.find((f) => f.path === p)?.status?.startsWith("merging"),
		);
		if (paths.length === 0) return;
		selected.value.clear();
		paths.sort((a, b) => {
			const fa = files.value.find((f) => f.path === a);
			const fb = files.value.find((f) => f.path === b);
			return (
				new Date(fa?.started_at ?? 0).getTime() -
				new Date(fb?.started_at ?? 0).getTime()
			);
		});
		for (const path of paths) {
			await call("run_postprocess_cmd", { path }).catch((e) => {
				toast(String(e), "error");
			});
		}
	}

	/** 已选中文件中可触发后处理的数量 / Number of selected files eligible for post-processing */
	const ppSelectableCount = computed(
		() =>
			[...selected.value].filter(
				(p) =>
					ppStatus.value[p] !== "running" &&
					ppStatus.value[p] !== "waiting" &&
					!files.value.find((f) => f.path === p)?.is_recording &&
					!files.value.find((f) => f.path === p)?.status?.startsWith("merging"),
			).length,
	);

	/** 所有正在录制文件的总录制速度（字节/秒）/ Total recording speed (bytes/second) */
	const totalRecordingSpeed = computed(() =>
		Object.values(recordingSpeed.value).reduce((sum, s) => sum + s, 0),
	);

	/** 正在录制的文件数量 / Number of files currently recording */
	const recordingCount = computed(
		() => files.value.filter((f) => f.is_recording).length,
	);

	/** 磁盘使用率百分比 / Disk usage percentage */
	const diskUsedPct = computed(() => {
		if (!diskSpace.value || diskSpace.value.total_bytes === 0) return 0;
		return Math.min(
			100,
			(diskSpace.value.used_bytes / diskSpace.value.total_bytes) * 100,
		);
	});
	onMounted(async () => {
		document.addEventListener("mousemove", onDocMousemove);
		document.addEventListener("mouseup", onDocMouseup);

		await load();
		startTick();
		await refreshDiskSpace();
		// 恢复仍在进行的手动发送，刷新页面后按钮上的进度不会丢
		// Restore in-flight manual sends so the row spinner survives a page reload
		tg.restore();
		const diskTimer = setInterval(refreshDiskSpace, 30_000);
		unlisteners.push(() => clearInterval(diskTimer));
		if (!ppStore.pipeline?.nodes?.length) await ppStore.fetchPipeline();

		// 先恢复运行中/等待中的后处理任务状态（来自内存，不依赖 meta）
		// First restore running/waiting post-processing task states (from memory, independent of meta)
		await restoreFromBackend();

		// 再从文件列表的 meta status 字段初始化 done/error 状态和模块输出路径。
		// meta 是持久化的真相来源，优先级高于推断值，直接覆盖写入。
		//
		// Then initialize status and module output paths from meta status fields in the file list.
		// Meta is the persistent source of truth and takes priority over inferred values.
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.status === "finish") {
				ppStatus.value[f.path] = "done";
			} else if (f.status === "pp_error") {
				ppStatus.value[f.path] = "error";
			} else if (f.status === "pp_waiting") {
				if (ppStatus.value[f.path] !== "running") ppStatus.value[f.path] = "waiting";
			} else if (f.status === "pp_running") {
				ppStatus.value[f.path] = "running";
			}
			// 从 meta pp_results 恢复各模块执行结果，用于 done/error 状态下的详情展示
			// Restore per-module results from meta pp_results for detail display in done/error state
			if (f.pp_results && f.pp_results.length > 0 &&
				(f.status === "finish" || f.status === "pp_error")) {
				const results = f.pp_results.map((r) => ({
					moduleId: r.module_id,
					success: r.success,
					message: r.message,
				}));
				const allOk = results.every((r) => r.success);
				ppProgress.value[f.path] = {
					...makePpProgress(
						allOk ? results.length : 0,
						results.length,
						0, 0, "", allOk ? 100 : 0, "", 0,
						{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
					),
					moduleResults: results,
				};
			}
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				moduleOutputs.value[f.path] = f.module_outputs;
			}
		}

		unlisteners.push(
			await on("recordings-dir-changed", () => scheduleDirRefresh(syncModuleOutputsFromFiles)),
		);

		unlisteners.push(
			await on("sse-lagged", async () => {
				// SSE 广播队列溢出，事件已丢失，重新从后端恢复完整状态
				// SSE broadcast queue overflowed, events lost; restore full state from backend
				await load();
				// 先恢复运行中任务，再用 meta 覆盖 done/error 状态
				// First restore running tasks, then overwrite done/error status from meta
				await restoreFromBackend();
				for (const f of files.value) {
					if (f.is_recording) continue;
					if (f.status === "finish") {
						ppStatus.value[f.path] = "done";
					} else if (f.status === "pp_error") {
						ppStatus.value[f.path] = "error";
					} else if (f.status === "pp_waiting") {
						if (ppStatus.value[f.path] !== "running") ppStatus.value[f.path] = "waiting";
					} else if (f.status === "pp_running") {
						ppStatus.value[f.path] = "running";
					}
					if (f.pp_results && f.pp_results.length > 0 &&
						(f.status === "finish" || f.status === "pp_error")) {
						const results = f.pp_results.map((r) => ({
							moduleId: r.module_id,
							success: r.success,
							message: r.message,
						}));
						const allOk = results.every((r) => r.success);
						ppProgress.value[f.path] = {
							...makePpProgress(
								allOk ? results.length : 0,
								results.length,
								0, 0, "", allOk ? 100 : 0, "", 0,
								{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
							),
							moduleResults: results,
						};
					}
					if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
						moduleOutputs.value[f.path] = f.module_outputs;
					}
				}
			}),
		);

		unlisteners.push(
			await on("recording-deleted", (payload) => {
				const p = payload as { path: string };
				const isLocal = localDeletedPaths.has(p.path);
				localDeletedPaths.delete(p.path);
				files.value = files.value.filter((r) => r.path !== p.path);
				delete elapsed.value[p.path];
				ppRemoveFile(p.path);
				selected.value.delete(p.path);
				if (!files.value.some((f) => f.is_recording)) stopTick();
				if (!isLocal) {
					const name = p.path.split(/[\\/]/).pop() ?? p.path;
					toast(t("recordings.otherClientDeleted", { name }), "info");
				}
			}),
		);

		unlisteners.push(
			await on("recording-file-update", async (payload) => {
				const p = payload as {
					path: string;
					size_bytes: number;
					speed_bps?: number;
				};
				// path is the video file path (from meta)
				const f = files.value.find((r) => r.path === p.path);
				if (f) {
					if (p.speed_bps != null && f.is_recording) {
						recordingSpeed.value = {
							...recordingSpeed.value,
							[p.path]: p.speed_bps,
						};
					} else if (!f.is_recording) {
						delete recordingSpeed.value[p.path];
					}
					f.size_bytes = p.size_bytes;
				} else {
					await load();
					startTick();
					syncModuleOutputsFromFiles();
				}
			}),
		);

		unlisteners.push(
			await on("recording-started", async () => {
				await load();
				startTick();
				syncModuleOutputsFromFiles();
			}),
		);

		unlisteners.push(
			await on("recording-stopped", async (payload) => {
				const p = payload as { video_path?: string };
				await load();
				syncModuleOutputsFromFiles();
				// 录制结束时清理速度数据 / Clean up recording speed when recording stops
				if (p.video_path) {
					const nextSpeed = { ...recordingSpeed.value };
					delete nextSpeed[p.video_path];
					recordingSpeed.value = nextSpeed;
				}
				// 合并完成后清理进度数据 / Clean up merge progress after merge completes
				if (p.video_path) {
					const next = { ...mergeProgress.value };
					delete next[p.video_path];
					mergeProgress.value = next;
				}
			}),
		);

		unlisteners.push(
			await on("merge-progress", (payload) => {
				const p = payload as {
					video_path: string;
					out_bytes: number;
					total_bytes: number;
				};
				if (p.video_path) {
					mergeProgress.value = {
						...mergeProgress.value,
						[p.video_path]: { out_bytes: p.out_bytes, total_bytes: p.total_bytes },
					};
				}
			}),
		);

		unlisteners.push(
			await on("postprocess-waiting", (payload) => {
				const p = payload as { path: string };
				ppStatus.value[p.path] = "waiting";
			}),
		);

		unlisteners.push(
			await on("postprocess-started", (payload) => {
				const p = payload as { path: string };
				ppStatus.value[p.path] = "running";
				ppProgress.value[p.path] = makePpProgress(0, 0, 0, 0, "", 0, "", 0, {
					processing: t("usePostprocess.processing"),
					waiting: t("usePostprocess.waitingProgress"),
				});
			}),
		);

		unlisteners.push(
			await on("postprocess-progress", (payload) => {
				const p = payload as {
					path: string;
					done: number;
					total: number;
					pct: number;
					modDone: number;
					modTotal: number;
					moduleName: string;
				};
				const prev = ppProgress.value[p.path];
				ppProgress.value[p.path] = makePpProgress(
					p.done,
					p.total,
					p.modDone,
					p.modTotal,
					p.moduleName ?? "",
					p.pct,
					prev?.moduleName ?? "",
					prev?.modulePct ?? 0,
					{
						processing: t("usePostprocess.processing"),
						waiting: t("usePostprocess.waitingProgress"),
					},
				);
			}),
		);

		unlisteners.push(
			await on("postprocess-done", async (payload) => {
				const p = payload as {
					path: string;
					results: { moduleId: string; success: boolean; message: string }[];
				};
				const wasCancelledByDelete = ppCancelledByDelete.has(p.path);
				ppCancelledByDelete.delete(p.path);
				handlePostprocessDone(
					p,
					async () => {
						await load();
						syncModuleOutputsFromFiles();
					},
					() => wasCancelledByDelete,
				);
			}),
		);

		// 上传成功后服务器上的数据被清空：把该行从列表里移除并提示释放的空间
		// The server data was wiped after a successful upload: drop the row and report the space freed
		unlisteners.push(
			tg.onPurged((path, freedBytes) => {
				files.value = files.value.filter((r) => r.path !== path);
				delete elapsed.value[path];
				selected.value.delete(path);
				ppRemoveFile(path);
				tg.clear(path);
				toast(
					t("telegram.purged", {
						name: path.split(/[\\/]/).pop() ?? path,
						size: formatSize(freedBytes),
					}),
					"success",
				);
				load();
			}),
		);
	});

	onUnmounted(() => {
		document.removeEventListener("mousemove", onDocMousemove);
		document.removeEventListener("mouseup", onDocMouseup);
		recCleanup();
		unlisteners.forEach((fn) => fn());
	});

	/** 顶部 header 元素引用，用于动态计算表头 sticky 偏移 */
	const headerEl = ref<HTMLElement | null>(null);
	const headerHeight = ref(0);
	let headerRo: ResizeObserver | null = null;
	watchEffect(() => {
		headerRo?.disconnect();
		if (!headerEl.value) return;
		headerRo = new ResizeObserver((entries) => {
			headerHeight.value = entries[0].borderBoxSize[0].blockSize;
		});
		headerRo.observe(headerEl.value);
	});
	onUnmounted(() => headerRo?.disconnect());
</script>

<template>
	<div class="flex flex-col">
		<!-- 手动发送到 Telegram / Manual send to Telegram -->
		<SendToTelegramDialog
			v-if="tgDialogPaths"
			:paths="tgDialogPaths"
			@close="tgDialogPaths = null"
		/>
		<Dialog :open="previewOpen" @update:open="previewOpen = $event">
			<!-- 手机端接近全屏，桌面端按图片比例自适应 / Near full-screen on phones, sized to the image on desktop -->
			<DialogContent
				class="p-0 overflow-y-auto overflow-x-hidden flex flex-col w-fit max-w-[calc(100vw-1rem)] dialog-max-h md:max-w-[90vw]"
			>
				<DialogHeader class="px-3 pt-3 pb-2 md:px-4 md:pt-4 shrink-0">
					<DialogTitle class="text-sm font-mono truncate pr-8">{{
						previewTitle
					}}</DialogTitle>
				</DialogHeader>
				<div
					ref="previewViewportRef"
					class="relative overflow-hidden flex items-center justify-center bg-black/5 px-2 pb-2 md:px-4 md:pb-4"
					:style="{
						width: viewportSize.width,
						height: viewportSize.height,
						maxWidth: '100%',
						cursor: isDragging
							? 'grabbing'
							: previewScale > 1
								? 'grab'
								: 'default',
					}"
					@wheel.prevent="onPreviewWheel"
					@mousedown="onPreviewMousedown"
				>
					<img
						ref="previewImageRef"
						:src="previewUrl"
						:alt="previewTitle"
						class="rounded select-none pointer-events-none"
						@load="onPreviewImageLoad"
						:style="{
							maxWidth: '100%',
							maxHeight: '100%',
							transform: `translate(${previewTranslate.x}px, ${previewTranslate.y}px) scale(${previewScale})`,
							transformOrigin: 'center center',
							transition: isDragging ? 'none' : 'transform 0.1s',
						}"
					/>
					<Transition name="fade">
						<Button
							v-if="previewScale !== 1"
							variant="secondary"
							size="sm"
							class="absolute bottom-5 left-1/2 -translate-x-1/2 z-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xs px-3 py-1.5 backdrop-blur-sm"
							@click="resetPreviewTransform"
						>
							{{
								t("recordings.resetZoom", {
									pct: Math.round(previewScale * 100),
								})
							}}
						</Button>
					</Transition>
				</div>
			</DialogContent>
		</Dialog>

		<header
			ref="headerEl"
			class="flex flex-col gap-3 md:flex-row md:items-start md:justify-between md:gap-4 shrink-0 pb-4 bg-background sticky top-0 z-20"
		>
			<div class="flex-1 min-w-0">
				<h1 class="text-xl font-bold mb-0.5">{{ t("recordings.title") }}</h1>
				<div
					class="flex items-center gap-3 text-sm text-muted-foreground flex-wrap"
				>
					<span>{{
						t("recordings.subtitle.total", { count: files.length })
					}}</span>
					<span v-if="recordingCount > 0" class="text-destructive">{{
						t("recordings.subtitle.recording", { count: recordingCount })
					}}</span>
					<span v-if="selectedCount > 0" class="text-foreground">{{
						t("recordings.subtitle.selected", { count: selectedCount })
					}}</span>
					<span v-if="totalRecordingSpeed > 0">
						{{ t("recordings.subtitle.totalSpeed") }}
						<span class="text-foreground tabular-nums"
							>{{ formatSize(totalRecordingSpeed) }}/s</span
						>
					</span>
				</div>
				<div v-if="diskSpace" class="mt-2 flex items-center gap-2 max-w-xs">
					<Progress
						:model-value="diskUsedPct"
						class="h-1.5 flex-1"
						:class="
							diskSpace.available_bytes < 5 * 1024 ** 3
								? '[&>div]:bg-destructive'
								: ''
						"
					/>
					<span
						class="text-xs text-muted-foreground whitespace-nowrap tabular-nums"
						:class="
							diskSpace.available_bytes < 5 * 1024 ** 3
								? 'text-destructive'
								: ''
						"
					>
						{{ formatSize(diskSpace.used_bytes) }} /
						{{ formatSize(diskSpace.total_bytes) }}
					</span>
				</div>
			</div>
			<!-- 桌面端批量操作；移动端改用底部固定操作栏 / Desktop batch actions; mobile uses the fixed bottom bar -->
			<div class="hidden md:flex gap-2 shrink-0">
				<Button
					v-if="selectedCount > 0"
					variant="outline"
					size="sm"
					:disabled="ppSelectableCount === 0"
					@click="postProcessSelected"
				>
					{{ t("recordings.batchPostprocess", { count: ppSelectableCount }) }}
				</Button>
				<Button
					v-if="selectedCount > 0"
					variant="outline"
					size="sm"
					class="gap-1.5"
					:disabled="tgSelectablePaths.length === 0"
					@click="sendSelectedToTelegram"
				>
					<Send class="size-3.5" />
					{{ t("telegram.sendSelected", { count: tgSelectablePaths.length }) }}
				</Button>
				<Button
					v-if="selectedCount > 0"
					variant="destructive"
					size="sm"
					@click="deleteSelected"
				>
					{{ t("recordings.deleteSelected", { count: selectedCount }) }}
				</Button>
			</div>
		</header>

		<!-- 移动端工具栏：全选 + 排序（表头在移动端隐藏）/ Mobile toolbar: select all + sorting (header hidden on mobile) -->
		<div
			v-if="files.length > 0"
			class="md:hidden flex flex-wrap items-center gap-x-2 gap-y-1 pb-3"
		>
			<!-- label 包裹复选框：整块可点，高度 44px / Label wraps the checkbox: whole block tappable, 44px tall -->
			<label
				class="flex items-center gap-2 shrink-0 min-h-11 pr-2 cursor-pointer select-none"
			>
				<Checkbox
					:model-value="getAllChecked()"
					class="size-5"
					@update:model-value="setAllChecked"
				/>
				<span class="text-xs text-muted-foreground">{{
					t("recordings.selectAll")
				}}</span>
			</label>
			<Button
				v-for="col in sortableColumns"
				:key="col.key"
				variant="outline"
				size="sm"
				class="shrink-0 gap-1 text-xs px-2"
				@click="toggleSort(col.key)"
			>
				{{ t(col.labelKey) }}
				<component :is="sortIcon(col.key)" class="size-3.5" />
			</Button>
		</div>

		<!-- 移动端批量操作栏（选中文件时出现，位于底部标签栏之上）
		     Mobile batch bar (shown when files are selected, sits above the bottom tab bar) -->
		<Transition name="fade">
			<div
				v-if="selectedCount > 0"
				class="md:hidden fixed inset-x-0 bottom-[calc(3.5rem_+_env(safe-area-inset-bottom))] z-30 flex gap-2 border-t border-border bg-background/95 px-3 py-2 backdrop-blur-sm"
			>
				<Button
					variant="outline"
					size="sm"
					class="flex-1 text-xs px-2"
					:disabled="ppSelectableCount === 0"
					@click="postProcessSelected"
				>
					{{ t("recordings.batchPostprocess", { count: ppSelectableCount }) }}
				</Button>
				<Button
					variant="outline"
					size="sm"
					class="flex-1 text-xs px-2 gap-1"
					:disabled="tgSelectablePaths.length === 0"
					@click="sendSelectedToTelegram"
				>
					<Send class="size-3.5 shrink-0" />
					<span class="truncate">{{ t("telegram.sendShort") }}</span>
				</Button>
				<Button
					variant="destructive"
					size="sm"
					class="flex-1 text-xs px-2"
					@click="deleteSelected"
				>
					{{ t("recordings.deleteSelected", { count: selectedCount }) }}
				</Button>
			</div>
		</Transition>

		<div :class="selectedCount > 0 ? 'pb-24 md:pb-6' : 'pb-6'">
			<div
				v-if="loading && files.length === 0"
				class="text-center text-muted-foreground py-16"
			>
				{{ t("recordings.loading") }}
			</div>
			<div
				v-else-if="files.length === 0"
				class="text-center text-muted-foreground py-16"
			>
				{{ t("recordings.empty") }}
			</div>

			<!--
				桌面端为普通表格；移动端（<md）把 table/tr/td 改为块级网格，
				每行渲染成一张卡片，无需重复一套标记。
				Desktop renders a normal table; on mobile (<md) table/tr/td switch to a block grid
				so each row becomes a card without duplicating the markup.
			-->
			<Table v-else class="max-md:block">
				<TableHeader
					class="sticky z-10 bg-background max-md:hidden"
					:style="{ top: `${headerHeight}px` }"
				>
					<TableRow>
						<TableHead class="w-8">
							<Checkbox
								:model-value="getAllChecked()"
								@update:model-value="setAllChecked"
							/>
						</TableHead>
						<TableHead class="w-px whitespace-nowrap">{{
							t("recordings.table.filename")
						}}</TableHead>
						<TableHead
							class="cursor-pointer select-none whitespace-nowrap"
							@click="toggleSort('size_bytes')"
						>
							{{ t("recordings.table.size") }}
							<component
								:is="sortIcon('size_bytes')"
								class="inline size-3.5 ml-0.5"
							/>
						</TableHead>
						<TableHead
							class="cursor-pointer select-none whitespace-nowrap"
							@click="toggleSort('started_at')"
						>
							{{ t("recordings.table.startTime") }}
							<component
								:is="sortIcon('started_at')"
								class="inline size-3.5 ml-0.5"
							/>
						</TableHead>
						<TableHead>{{ t("recordings.table.recordDuration") }}</TableHead>
						<TableHead
							class="cursor-pointer select-none whitespace-nowrap"
							@click="toggleSort('video_duration_secs')"
						>
							{{ t("recordings.table.videoDuration") }}
							<component
								:is="sortIcon('video_duration_secs')"
								class="inline size-3.5 ml-0.5"
							/>
						</TableHead>
						<TableHead>{{ t("recordings.table.speed") }}</TableHead>
						<TableHead class="min-w-45">{{
							t("recordings.table.postprocess")
						}}</TableHead>
						<TableHead>{{ t("recordings.table.actions") }}</TableHead>
					</TableRow>
				</TableHeader>
				<TableBody class="max-md:block max-md:space-y-3">
					<template v-for="group in groups" :key="group.username">
						<!-- 分组标题行 / Group header row -->
						<TableRow
							class="bg-muted/40 hover:bg-muted/60 cursor-pointer max-md:flex max-md:flex-wrap max-md:items-center max-md:gap-x-2 max-md:rounded-lg max-md:border max-md:px-2 max-md:py-1"
							@click="toggleGroup(group.username)"
						>
							<TableCell class="w-8 max-md:w-auto max-md:p-0" @click.stop>
								<Checkbox
									:model-value="getGroupChecked(group)"
									@update:model-value="setGroupChecked(group)"
								/>
							</TableCell>
							<TableCell
								colspan="7"
								class="font-semibold max-md:flex-1 max-md:min-w-0 max-md:p-0 max-md:py-1 max-md:whitespace-normal"
							>
								<span class="mr-2 text-muted-foreground text-xs">{{
									collapsedGroups.has(group.username) ? "▶" : "▼"
								}}</span>
								{{ group.username }}
								<Badge
									v-if="group.hasRecording"
									variant="destructive"
									class="ml-2 text-[10px]"
									>{{ t("recordings.status.recording") }}</Badge
								>
								<span class="ml-2 text-xs text-muted-foreground font-normal">
									{{
										t("recordings.group.fileCount", {
											count: group.files.length,
										})
									}}
									·
									{{ formatSize(group.totalSize) }}
								</span>
							</TableCell>
							<TableCell class="max-md:hidden" />
						</TableRow>

						<template v-if="!collapsedGroups.has(group.username)">
							<TableRow
								v-for="f in group.files"
								:key="f.path"
								class="relative max-md:grid max-md:grid-cols-[auto_1fr] max-md:items-center max-md:gap-x-3 max-md:gap-y-1 max-md:rounded-lg max-md:border max-md:bg-card max-md:p-3"
							>
								<template v-if="f.status === 'merging_waiting' || f.status === 'merging'">
									<TableCell class="w-8 max-md:w-auto max-md:p-0">
										<Checkbox :model-value="false" :disabled="true" />
									</TableCell>
									<TableCell
										class="font-medium w-px whitespace-nowrap pl-7 max-md:w-auto max-md:pl-0 max-md:whitespace-normal"
									>
										<div class="flex items-center gap-1.5">
											<span class="max-md:break-all">{{ f.name }}</span>
											<Badge variant="outline" class="text-[10px] shrink-0">{{
												f.status === 'merging_waiting'
													? t("recordings.status.waitingMerge")
													: t("recordings.status.merging")
											}}</Badge>
										</div>
									</TableCell>
									<td
										colspan="7"
										class="p-2 align-middle w-full max-md:col-span-2 max-md:w-auto max-md:p-0"
									>
										<div
											class="flex items-center gap-3 h-9 w-full max-md:h-auto max-md:flex-wrap max-md:gap-2"
										>
											<Loader2
												class="size-4 animate-spin shrink-0 text-muted-foreground"
											/>
											<span class="text-xs text-muted-foreground shrink-0">{{
												f.status === 'merging_waiting'
													? t("recordings.status.waitingMergeVideo")
													: t("recordings.status.mergingVideo")
											}}</span>
											<template v-if="f.status === 'merging' && mergeProgress[f.path]">
												<div
													class="flex-1 bg-muted rounded-full h-1.5 overflow-hidden"
												>
													<div
														class="h-full bg-primary rounded-full transition-all duration-500"
														:style="{
															width: `${mergeProgress[f.path].total_bytes > 0
																? Math.min(99, Math.floor(mergeProgress[f.path].out_bytes / mergeProgress[f.path].total_bytes * 10000) / 100)
																: 0}%`,
														}"
													/>
												</div>
												<span class="tabular-nums text-xs text-muted-foreground w-14 shrink-0">{{
													mergeProgress[f.path].total_bytes > 0
														? (Math.min(99, Math.floor(mergeProgress[f.path].out_bytes / mergeProgress[f.path].total_bytes * 10000) / 100)).toFixed(2)
														: '0.00'
												}}%</span>
											</template>
										</div>
									</td>
								</template>

								<template v-else>
									<TableCell class="w-8 max-md:w-auto max-md:p-0">
										<Checkbox
											:model-value="getFileChecked(f.path)"
											:disabled="f.is_recording"
											@update:model-value="setFileChecked(f.path)"
										/>
									</TableCell>
									<TableCell
										class="font-medium w-px whitespace-nowrap pl-7 max-md:w-auto max-md:pl-0 max-md:whitespace-normal max-md:break-all"
									>
										{{ f.name }}
										<Badge
											v-if="f.is_recording"
											variant="destructive"
											class="ml-1.5 text-[10px]"
											>{{ t("recordings.status.recording") }}</Badge
										>
									</TableCell>
									<TableCell
										class="tabular-nums max-md:col-span-2 max-md:p-0 max-md:text-xs"
									>
										<span class="md:hidden text-muted-foreground"
											>{{ t("recordings.table.size") }}: </span
										>{{ formatSize(f.size_bytes) }}
									</TableCell>
									<TableCell
										class="tabular-nums text-muted-foreground max-md:col-span-2 max-md:p-0 max-md:text-xs"
									>
										<span class="md:hidden"
											>{{ t("recordings.table.startTime") }}: </span
										>{{ new Date(f.started_at).toLocaleString() }}
									</TableCell>
									<TableCell
										class="tabular-nums max-md:col-span-2 max-md:p-0 max-md:text-xs"
									>
										<span class="md:hidden text-muted-foreground"
											>{{ t("recordings.table.recordDuration") }}: </span
										>
										<span v-if="f.is_recording" class="text-destructive">{{
											formatDuration(elapsed[f.path] ?? 0)
										}}</span>
										<span v-else class="text-muted-foreground">—</span>
									</TableCell>
									<TableCell
										class="tabular-nums max-md:col-span-2 max-md:p-0 max-md:text-xs"
									>
										<span class="md:hidden text-muted-foreground"
											>{{ t("recordings.table.videoDuration") }}: </span
										>
										<span v-if="f.video_duration_secs != null">{{
											formatDuration(f.video_duration_secs)
										}}</span>
										<span v-else class="text-muted-foreground">—</span>
									</TableCell>
									<TableCell
										class="tabular-nums max-md:col-span-2 max-md:p-0 max-md:text-xs"
									>
										<span class="md:hidden text-muted-foreground"
											>{{ t("recordings.table.speed") }}: </span
										>
										<span
											v-if="f.is_recording && recordingSpeed[f.path] != null"
											class="text-xs"
										>
											{{ formatSize(recordingSpeed[f.path]) }}/s
										</span>
										<span v-else class="text-muted-foreground">—</span>
									</TableCell>
									<TableCell class="min-w-45 max-md:col-span-2 max-md:min-w-0 max-md:p-0">
										<div class="md:hidden text-xs text-muted-foreground mb-1">{{
											t("recordings.table.postprocess")
										}}</div>
										<div v-if="!f.is_recording">
											<div
												v-if="
													ppStatus[f.path] === 'running' && ppProgress[f.path]
												"
												class="flex flex-col gap-1.5"
											>
												<div
													class="flex items-center justify-between text-xs text-muted-foreground"
												>
													<span>{{
														ppProgress[f.path].moduleExecLabel
															? t(
																	"recordings.status.overallProgressWithLabel",
																	{ label: ppProgress[f.path].moduleExecLabel },
																)
															: t("recordings.status.overallProgress")
													}}</span>
													<span class="tabular-nums shrink-0">{{
														ppProgress[f.path].overallLabel
													}}</span>
												</div>
												<Progress
													:model-value="ppProgress[f.path].overallPct"
													:animated="false"
													class="h-1.5"
												/>
												<div
													class="flex items-center justify-between text-xs text-muted-foreground"
												>
													<span class="truncate max-w-50">{{
														ppProgress[f.path].moduleName === "processing"
															? t("usePostprocess.processing")
															: ppProgress[f.path].moduleName
													}}</span>
													<span class="tabular-nums shrink-0">{{
														ppProgress[f.path].moduleLabel === "waiting"
															? t("usePostprocess.waitingProgress")
															: ppProgress[f.path].moduleLabel
													}}</span>
												</div>
												<Progress
													:model-value="ppProgress[f.path].modulePct"
													:animated="false"
													class="h-1.5"
												/>
											</div>
											<div
												v-else-if="ppStatus[f.path] === 'waiting'"
												class="flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<Loader2 class="size-3 animate-spin shrink-0" />
												<span>{{ t("recordings.status.waiting") }}</span>
											</div>
											<div
												v-else-if="ppStatus[f.path] === 'done' || ppStatus[f.path] === 'error'"
												class="flex flex-col gap-0.5"
											>
												<template v-if="ppProgress[f.path]?.moduleResults?.length">
													<div
														v-for="r in ppProgress[f.path].moduleResults"
														:key="r.moduleId"
														class="flex items-center gap-1.5 text-xs"
														:class="r.success ? 'text-green-500' : 'text-destructive'"
														:title="r.success ? r.moduleId : `${r.moduleId}: ${r.message}`"
													>
														<span class="shrink-0">{{ r.success ? "✓" : "✗" }}</span>
														<span class="truncate max-w-40">{{ r.moduleId }}</span>
													</div>
												</template>
												<template v-else>
													<div
														v-if="ppStatus[f.path] === 'done'"
														class="text-lg text-green-500"
													>
														{{ t("recordings.status.done") }}
													</div>
													<div v-else class="text-lg text-destructive">
														{{ t("recordings.status.failed") }}
													</div>
												</template>
											</div>
											<span v-else class="text-xs text-muted-foreground"
												>—</span
											>
										</div>
										<span
											v-if="f.is_recording"
											class="text-xs text-muted-foreground"
											>—</span
										>
									</TableCell>
									<TableCell class="max-md:col-span-2 max-md:p-0 max-md:pt-1">
										<div class="flex flex-wrap gap-1.5">
											<Button
												size="sm"
												variant="outline"
												:disabled="f.is_recording"
												:title="
													f.is_recording
														? t('recordings.actions.playDisabled')
														: ''
												"
												@click="openFile(f.path)"
												>{{ t("recordings.actions.play") }}</Button
											>
											<Button
												v-if="moduleOutputs[f.path]?.['contact_sheet']"
												size="sm"
												variant="outline"
												:title="t('recordings.actions.preview')"
												:aria-label="t('recordings.actions.preview')"
												@click="openModuleOutput(f.path, 'contact_sheet')"
											>
												<Image class="size-3.5" />
											</Button>
											<Button
												size="sm"
												variant="outline"
												:disabled="
													f.is_recording ||
													ppStatus[f.path] === 'running' ||
													ppStatus[f.path] === 'waiting'
												"
												:title="
													f.is_recording ? t('recordings.status.recording') : ''
												"
												@click="runPostprocess(f.path)"
											>
												<Loader2
													v-if="ppStatus[f.path] === 'running'"
													class="size-3.5 animate-spin"
												/>
												<span v-else>{{
													t("recordings.actions.postprocess")
												}}</span>
											</Button>
											<Button
												size="sm"
												variant="outline"
												:title="t('telegram.sendOne')"
												:aria-label="t('telegram.sendOne')"
												:disabled="
													f.is_recording ||
													f.status?.startsWith('merging') ||
													tg.isSending(f.path)
												"
												@click="openTelegramSend([f.path])"
											>
												<Loader2
													v-if="tg.isSending(f.path)"
													class="size-3.5 animate-spin"
												/>
												<Send v-else class="size-3.5" />
											</Button>
											<Button
												size="sm"
												variant="destructive"
												:disabled="f.is_recording"
												:title="
													f.is_recording
														? t('recordings.actions.deleteDisabled')
														: ''
												"
												@click="deleteFile(f)"
												>{{ t("recordings.actions.delete") }}</Button
											>
										</div>
									</TableCell>
								</template>
							</TableRow>
						</template>
					</template>
				</TableBody>
			</Table>
		</div>
	</div>
</template>

<style scoped>
	.fade-enter-active,
	.fade-leave-active {
		transition: opacity 0.15s;
	}
	.fade-enter-from,
	.fade-leave-to {
		opacity: 0;
	}
</style>
