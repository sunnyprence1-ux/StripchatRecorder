/**
 * 手动发送到 Telegram 的状态管理 / Manual "send to Telegram" state management
 *
 * 后端通过 SSE 推送 `telegram-send-*` 事件，这里把它们汇总成每个文件一份的发送状态，
 * 供「发送到 Telegram」对话框和录制列表共享。事件监听只注册一次（模块级单例），
 * 页面切换不会重复订阅。
 *
 * The backend pushes `telegram-send-*` SSE events; this module aggregates them into one
 * send state per file, shared by the "send to Telegram" dialog and the recordings list.
 * The listeners are registered exactly once (module-level singleton), so switching pages
 * never double-subscribes.
 */

import { reactive } from "vue";
import { call, on } from "@/lib/api";

/** 单次手动发送的状态 / State of one manual send */
export interface TgSendState {
	/** 任务状态 / Task status */
	status: "idle" | "running" | "done" | "error" | "cancelled";
	/** 上传进度百分比（0-100）/ Upload progress percentage (0-100) */
	pct: number;
	/** 状态文字（上传速度等）/ Status text (upload speed etc.) */
	statusText: string;
	/** 结果消息 / Result message */
	message: string;
	/** 模块输出日志（最多保留最近若干行）/ Module output log (only the last few lines are kept) */
	logs: { stream: string; line: string }[];
	/** 成功后是否删除了本地数据 / Whether local data was deleted after success */
	deleted: boolean;
}

/** 日志最多保留的行数 / Maximum number of log lines kept per file */
const MAX_LOG_LINES = 300;

/** 每个文件路径对应的发送状态（模块级单例）/ Send state per file path (module-level singleton) */
const sendStates = reactive<Record<string, TgSendState>>({});

/** `recording-purged` 事件的订阅者 / Subscribers to the `recording-purged` event */
type PurgedHandler = (path: string, freedBytes: number) => void;
const purgedHandlers = new Set<PurgedHandler>();

/** 监听器是否已注册 / Whether the listeners are registered */
let listening = false;

/**
 * 取得（必要时创建）某个文件的发送状态对象。
 * Get (creating if needed) the send state object for a file.
 */
function stateFor(path: string): TgSendState {
	if (!sendStates[path]) {
		sendStates[path] = {
			status: "idle",
			pct: 0,
			statusText: "",
			message: "",
			logs: [],
			deleted: false,
		};
	}
	return sendStates[path];
}

/**
 * 注册所有 SSE 监听器（只会执行一次）。
 * Register all SSE listeners (runs only once).
 */
async function ensureListeners() {
	if (listening) return;
	listening = true;

	await on("telegram-send-started", (payload) => {
		const p = payload as { path: string };
		const s = stateFor(p.path);
		s.status = "running";
		s.pct = 0;
		s.statusText = "";
		s.message = "";
		s.logs = [];
		s.deleted = false;
	});

	await on("telegram-send-progress", (payload) => {
		const p = payload as { path: string; pct?: number; statusText?: string };
		const s = stateFor(p.path);
		if (typeof p.pct === "number" && Number.isFinite(p.pct)) {
			// 进度只增不减，避免重试时回跳 / Progress only moves forward, so retries don't jump back
			s.pct = Math.max(s.pct, Math.min(100, p.pct));
		}
		if (typeof p.statusText === "string" && p.statusText) {
			s.statusText = p.statusText;
		}
	});

	await on("telegram-send-log", (payload) => {
		const p = payload as { path: string; stream: string; line: string };
		const s = stateFor(p.path);
		s.logs.push({ stream: p.stream, line: p.line });
		if (s.logs.length > MAX_LOG_LINES) {
			s.logs.splice(0, s.logs.length - MAX_LOG_LINES);
		}
	});

	await on("telegram-send-done", (payload) => {
		const p = payload as {
			path: string;
			success: boolean;
			cancelled?: boolean;
			message?: string;
			deleted?: boolean;
		};
		const s = stateFor(p.path);
		// 取消是一种独立状态：界面要显示「已取消」而不是「失败」
		// Cancelling is its own state: the UI shows "cancelled" rather than "failed"
		s.status = p.success ? "done" : p.cancelled ? "cancelled" : "error";
		s.message = p.message ?? "";
		s.deleted = Boolean(p.deleted);
		if (p.success) s.pct = 100;
	});

	await on("recording-purged", (payload) => {
		const p = payload as { path: string; freedBytes?: number };
		const s = stateFor(p.path);
		s.deleted = true;
		for (const cb of purgedHandlers) cb(p.path, p.freedBytes ?? 0);
	});
}

/**
 * 手动发送的参数 / Manual-send options
 */
export interface TgSendOptions {
	/** 群组 ID（留空则用流水线里的配置）/ Chat id (empty = the pipeline's value) */
	chatId?: string;
	/** 群组 username / Group username */
	username?: string;
	/** 按模特分话题 / One topic per model */
	topicPerModel?: boolean;
	/** 发送成功后删除本地数据 / Delete local data after a successful send */
	deleteAfterSend?: boolean;
	/** 同时发送视频文件 / Also send the video file */
	sendVideo?: boolean;
}

/**
 * 手动发送的默认值（来自流水线节点）。
 * Defaults for manual sending (taken from the pipeline node).
 */
export interface TgSendDefaults {
	available: boolean;
	moduleName: string;
	chatId: string;
	username: string;
	topicPerModel: boolean;
	deleteAfterSend: boolean;
	sendVideo: boolean;
	account: string;
	hasAccount: boolean;
	inPipeline: boolean;
}

/**
 * 读取手动发送的默认值。
 * Fetch the manual-send defaults.
 */
export async function fetchTelegramDefaults(): Promise<TgSendDefaults> {
	return call<TgSendDefaults>("telegram_defaults");
}

/**
 * 发送一个或多个视频到 Telegram（后端串行执行）。
 * Send one or more videos to Telegram (the backend runs them one at a time).
 *
 * @param paths - 要发送的视频路径列表 / Video paths to send
 * @param opts - 覆盖参数 / Override options
 */
export async function sendToTelegram(paths: string[], opts: TgSendOptions = {}) {
	await ensureListeners();
	for (const path of paths) {
		const s = stateFor(path);
		s.status = "running";
		s.pct = 0;
		s.statusText = "";
		s.message = "";
		s.logs = [];
		s.deleted = false;
		try {
			await call("telegram_send", { path, ...opts });
		} catch (e) {
			s.status = "error";
			s.message = String(e);
		}
	}
}

/**
 * 取消某个文件正在进行的发送。
 * Cancel the in-flight send for a file.
 */
export async function cancelTelegramSend(path: string) {
	await call("telegram_send_cancel", { path });
}

/**
 * 订阅「录制数据已被删除」事件（例如上传成功后清空服务器）。
 * 返回取消订阅函数。
 *
 * Subscribe to the "recording data was purged" event (e.g. wiped after an upload).
 * Returns an unsubscribe function.
 */
export function onRecordingPurged(cb: PurgedHandler): () => void {
	ensureListeners();
	purgedHandlers.add(cb);
	return () => purgedHandlers.delete(cb);
}

/**
 * 从后端恢复手动发送任务状态（页面刷新或换设备后仍能看到正在上传的文件）。
 * Restore manual-send task state from the backend (so a page reload or another device still
 * sees files that are currently uploading).
 */
export async function restoreTelegramTasks() {
	await ensureListeners();
	try {
		const res = await call<{
			tasks: {
				path: string;
				status: string;
				pct: number;
				statusText: string;
				message: string;
				deleteAfterSend: boolean;
			}[];
		}>("telegram_tasks");
		for (const task of res.tasks ?? []) {
			const s = stateFor(task.path);
			// 运行中的任务用后端数据覆盖；已完成的任务只在本地没有记录时补上
			// Running tasks are taken from the backend; finished ones only fill gaps locally
			if (task.status === "running" || s.status === "idle") {
				s.status = (task.status as TgSendState["status"]) ?? "idle";
				s.pct = typeof task.pct === "number" ? task.pct : s.pct;
				s.statusText = task.statusText || s.statusText;
				s.message = task.message || s.message;
			}
		}
	} catch {
		// 恢复失败不影响正常使用 / A failed restore doesn't block normal use
	}
}

/**
 * 手动发送相关的响应式状态与操作。
 * Reactive state and operations for manual sending.
 */
export function useTelegramSend() {
	ensureListeners();

	/** 某个文件的发送状态（不存在时返回 idle 快照）/ Send state for a file (idle snapshot when unknown) */
	function stateOf(path: string): TgSendState {
		return sendStates[path] ?? {
			status: "idle",
			pct: 0,
			statusText: "",
			message: "",
			logs: [],
			deleted: false,
		};
	}

	/** 该文件是否正在发送 / Whether that file is currently being sent */
	function isSending(path: string): boolean {
		return sendStates[path]?.status === "running";
	}

	/** 是否有任何文件正在发送 / Whether any file is currently being sent */
	function anySending(): boolean {
		return Object.values(sendStates).some((s) => s.status === "running");
	}

	/** 清除某个文件的状态 / Clear the state of one file */
	function clear(path: string) {
		delete sendStates[path];
	}

	return {
		sendStates,
		stateOf,
		isSending,
		anySending,
		clear,
		send: sendToTelegram,
		cancel: cancelTelegramSend,
		onPurged: onRecordingPurged,
		fetchDefaults: fetchTelegramDefaults,
		restore: restoreTelegramTasks,
	};
}
