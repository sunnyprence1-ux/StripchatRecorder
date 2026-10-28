<!--
    发送到 Telegram 对话框 / Send to Telegram Dialog

    手动把一个或多个成片发送到 Telegram 群组：群组 ID、是否按模特分话题、是否发送视频文件、
    是否附带文字说明、是否发送封面图，以及「发送成功后删除本地数据」都可以在这里覆盖，
    不必修改后处理流水线。

    Manually send one or more finished videos to a Telegram group. The group id, the
    one-topic-per-model toggle, whether to send the video file, whether to attach the text
    caption, whether to send the cover image, and "delete local data after a successful send"
    can all be overridden here without touching the post-processing pipeline.

    Props:
        paths - 要发送的视频路径列表 / Video paths to send
    Emits:
        close - 对话框关闭时触发 / Emitted when the dialog is closed
-->
<script setup lang="ts">
	import { computed, onMounted, reactive, ref } from "vue";
	import { useI18n } from "vue-i18n";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogHeader,
		DialogTitle,
		DialogFooter,
	} from "@/components/ui/dialog";
	import { Button } from "@/components/ui/button";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import { Switch } from "@/components/ui/switch";
	import { Progress } from "@/components/ui/progress";
	import { useNotify } from "@/composables/useNotify";
	import { useTelegramSend } from "@/composables/useTelegramSend";
	import { ChevronDown, Loader2, Send, Square, Trash2, TriangleAlert } from "@lucide/vue";

	const props = defineProps<{ paths: string[] }>();
	const emit = defineEmits<{ close: [] }>();

	const { t } = useI18n();
	const { toast } = useNotify();
	const tg = useTelegramSend();

	/** 表单参数 / Form values */
	const form = reactive({
		chatId: "",
		username: "",
		topicPerModel: true,
		deleteAfterSend: true,
		sendVideo: true,
		sendCaption: false,
		sendCover: true,
		coverFirst: true,
		/** 同时上传的文件数量（1 = 逐个，>1 = 并发）/ Concurrent upload count */
		batchSize: 3,
	});

	/** 是否正在加载默认值 / Whether the defaults are loading */
	const loading = ref(true);
	/** 模块是否可用 / Whether the module is available */
	const available = ref(true);
	/** 是否已有可用账号 / Whether an account is configured */
	const hasAccount = ref(true);
	/** 是否展开高级选项 / Whether the advanced section is expanded */
	const showAdvanced = ref(false);
	/** 是否已经提交过 / Whether the send has been submitted */
	const submitted = ref(false);

	/** 文件名（不含目录）/ File names without the directory */
	const fileNames = computed(() =>
		props.paths.map((p) => p.split(/[\\/]/).pop() ?? p),
	);

	/** 每个文件当前的发送状态 / Current send state per file */
	const states = computed(() => props.paths.map((p) => tg.stateOf(p)));

	/** 是否全部完成 / Whether every file has finished */
	const allFinished = computed(
		() =>
			submitted.value &&
			states.value.length > 0 &&
			states.value.every((s) => s.status !== "running"),
	);

	/** 成功数量 / Number of successes */
	const successCount = computed(
		() => states.value.filter((s) => s.status === "done").length,
	);

	/** 整体进度（按文件平均）/ Overall progress (averaged over files) */
	const overallPct = computed(() => {
		if (states.value.length === 0) return 0;
		const sum = states.value.reduce((acc, s) => {
			if (s.status === "done") return acc + 100;
			return acc + (Number.isFinite(s.pct) ? s.pct : 0);
		}, 0);
		return Math.round((sum / states.value.length) * 10) / 10;
	});

	/** 日志面板是否可见 / Whether the log panel is visible */
	const showLogs = computed(() => submitted.value && states.value.some((s) => s.logs.length > 0));

	/** 所有日志行（带文件名前缀，只保留最近 200 行）/ All log lines (prefixed with the file name, last 200 only) */
	const logLines = computed(() => {
		const out: { path: string; name: string; line: string; stream: string }[] = [];
		props.paths.forEach((p, i) => {
			const s = tg.stateOf(p);
			for (const l of s.logs) {
				out.push({ path: p, name: fileNames.value[i], line: l.line, stream: l.stream });
			}
		});
		return out.slice(-200);
	});

	/** 发送前是否需要提醒用户「数据会被删除」/ Whether to warn that data will be deleted */
	const willDelete = computed(() => form.deleteAfterSend);

	onMounted(async () => {
		try {
			const d = await tg.fetchDefaults();
			available.value = d.available;
			hasAccount.value = d.hasAccount;
			form.chatId = d.chatId ?? "";
			form.username = d.username ?? "";
			form.topicPerModel = d.topicPerModel;
			form.deleteAfterSend = d.deleteAfterSend;
			form.sendVideo = d.sendVideo;
			form.sendCaption = d.sendCaption ?? false;
			form.sendCover = d.sendCover ?? true;
			form.coverFirst = d.coverFirst ?? true;
			form.batchSize = d.batchSize && d.batchSize > 0 ? d.batchSize : 3;
		} catch {
			// 读取默认值失败不阻塞发送：用户仍可手动填写群组 ID
			// Failing to read defaults doesn't block sending: the user can still type a chat id
		} finally {
			loading.value = false;
		}
	});

	/** 是否正在发送 / Whether a send is in progress */
	const sending = computed(() => states.value.some((s) => s.status === "running"));

	/** 是否至少选择了一种内容（视频 / 文字 / 封面图）/ Whether at least one payload is selected */
	const hasPayload = computed(
		() => form.sendVideo || form.sendCaption || form.sendCover,
	);

	/** 是否可以提交 / Whether the form can be submitted */
	const canSubmit = computed(
		() =>
			!loading.value &&
			available.value &&
			!sending.value &&
			hasPayload.value &&
			(form.chatId.trim() !== "" || form.username.trim() !== ""),
	);

	/**
	 * 提交发送：把表单参数作为覆盖项传给后端。
	 * Submit the send, passing the form values as overrides.
	 */
	async function submit() {
		if (!canSubmit.value) return;
		submitted.value = true;
		await tg.send(props.paths, {
			chatId: form.chatId.trim(),
			username: form.username.trim(),
			topicPerModel: form.topicPerModel,
			deleteAfterSend: form.deleteAfterSend,
			sendVideo: form.sendVideo,
			sendCaption: form.sendCaption,
			sendCover: form.sendCover,
			coverFirst: form.coverFirst,
			batchSize: Math.max(1, Math.min(10, Math.floor(form.batchSize) || 1)),
		});
	}

	/**
	 * 取消所有正在进行的发送。
	 * Cancel every in-flight send.
	 */
	async function cancelAll() {
		for (const p of props.paths) {
			if (tg.isSending(p)) {
				await tg.cancel(p).catch(() => {});
			}
		}
		toast(t("telegram.cancelled"), "warning");
	}

	/**
	 * 关闭对话框；正在发送时不允许直接关闭，避免用户误以为已经取消。
	 * Close the dialog; while sending, close is blocked so it can't be mistaken for a cancel.
	 */
	function close() {
		if (sending.value) {
			toast(t("telegram.stillSending"), "warning");
			return;
		}
		emit("close");
	}
</script>

<template>
	<Dialog :open="true" @update:open="(v) => !v && close()">
		<DialogContent
			class="p-0 gap-0 flex flex-col w-[calc(100vw-1rem)] max-w-[34rem] dialog-max-h"
		>
			<DialogHeader class="px-4 pt-4 pb-3 shrink-0 border-b">
				<DialogTitle class="flex items-center gap-2 text-base">
					<Send class="size-4 shrink-0" />
					<span class="truncate">{{ t("telegram.sendTitle") }}</span>
				</DialogTitle>
				<DialogDescription class="text-xs">
					{{ t("telegram.sendDescription", { count: paths.length }) }}
				</DialogDescription>
			</DialogHeader>

			<!-- 可滚动主体 / Scrollable body -->
			<div class="flex-1 min-h-0 overflow-y-auto px-4 py-3 flex flex-col gap-4">
				<!-- 模块缺失 / Module missing -->
				<div
					v-if="!loading && !available"
					class="rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive"
				>
					{{ t("telegram.moduleMissing") }}
				</div>

				<!-- 没有可用账号 / No account -->
				<div
					v-else-if="!loading && !hasAccount"
					class="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs"
				>
					{{ t("telegram.noAccount") }}
				</div>

				<!-- 文件列表 / File list -->
				<div class="flex flex-col gap-2">
					<Label class="text-xs text-muted-foreground">{{ t("telegram.files") }}</Label>
					<div
						v-for="(name, i) in fileNames"
						:key="paths[i]"
						class="rounded-md border bg-muted/30 px-3 py-2 text-xs"
					>
						<div class="flex items-center justify-between gap-2">
							<span class="truncate font-mono">{{ name }}</span>
							<span
								v-if="states[i].status === 'running'"
								class="shrink-0 flex items-center gap-1 text-muted-foreground"
							>
								<Loader2 class="size-3 animate-spin" />
								{{ Math.round(states[i].pct) }}%
							</span>
							<span
								v-else-if="states[i].status === 'done'"
								class="shrink-0 text-emerald-600 dark:text-emerald-400"
								>{{ t("telegram.sent") }}</span
							>
							<span
								v-else-if="states[i].status === 'error'"
								class="shrink-0 text-destructive"
								>{{ t("telegram.failed") }}</span
							>
							<span
								v-else-if="states[i].status === 'cancelled'"
								class="shrink-0 text-muted-foreground"
								>{{ t("telegram.cancelledState") }}</span
							>
						</div>
						<!-- 进度条 / Progress bar -->
						<Progress
							v-if="states[i].status === 'running'"
							:model-value="states[i].pct"
							class="h-1 mt-2"
						/>
						<div
							v-if="states[i].statusText"
							class="mt-1 text-[11px] text-muted-foreground tabular-nums truncate"
						>
							{{ states[i].statusText }}
						</div>
						<div
							v-if="states[i].message && states[i].status !== 'done'"
							class="mt-1 text-[11px] text-destructive break-words"
						>
							{{ states[i].message }}
						</div>
					</div>
					<!-- 整体进度 / Overall progress -->
					<div v-if="sending || submitted" class="flex items-center gap-2 text-[11px] text-muted-foreground">
						<Progress :model-value="overallPct" class="h-1 flex-1" />
						<span class="tabular-nums shrink-0">{{ overallPct.toFixed(1) }}%</span>
					</div>
				</div>

				<!-- 表单（发送开始后隐藏）/ Form (hidden once sending starts) -->
				<template v-if="!submitted">
					<div class="flex flex-col gap-2">
						<Label for="tg-chat-id" class="text-xs">{{ t("telegram.chatId") }}</Label>
						<Input
							id="tg-chat-id"
							v-model="form.chatId"
							inputmode="numeric"
							:placeholder="t('telegram.chatIdPlaceholder')"
							:disabled="loading"
							class="font-mono"
						/>
						<p class="text-[11px] text-muted-foreground">{{ t("telegram.chatIdHint") }}</p>
					</div>

					<div class="flex flex-col gap-2">
						<Label for="tg-batch-size" class="text-xs">{{
							t("telegram.batchSize")
						}}</Label>
						<div class="flex items-center gap-2">
							<Input
								id="tg-batch-size"
								v-model.number="form.batchSize"
								type="number"
								inputmode="numeric"
								min="1"
								max="10"
								step="1"
								class="w-24"
								:disabled="loading || sending"
							/>
							<span class="text-[11px] text-muted-foreground">{{
								t("telegram.batchSizeHint")
							}}</span>
						</div>
					</div>

					<button
						type="button"
						class="flex items-center gap-1 text-xs text-muted-foreground self-start"
						@click="showAdvanced = !showAdvanced"
					>
						<ChevronDown
							class="size-3 transition-transform"
							:class="showAdvanced ? 'rotate-180' : ''"
						/>
						{{ t("telegram.advanced") }}
					</button>

					<div v-if="showAdvanced" class="flex flex-col gap-3 rounded-md border p-3">
						<div class="flex flex-col gap-2">
							<Label for="tg-username" class="text-xs">{{ t("telegram.username") }}</Label>
							<Input
								id="tg-username"
								v-model="form.username"
								:placeholder="t('telegram.usernamePlaceholder')"
								:disabled="loading"
							/>
						</div>
						<div class="flex items-center justify-between gap-3">
							<Label for="tg-send-video" class="text-xs">{{ t("telegram.sendVideo") }}</Label>
							<Switch id="tg-send-video" v-model="form.sendVideo" />
						</div>
						<div class="flex items-start justify-between gap-3">
							<div class="flex flex-col gap-0.5">
								<Label for="tg-send-cover" class="text-xs">{{ t("telegram.sendCover") }}</Label>
								<p class="text-[11px] text-muted-foreground">{{ t("telegram.sendCoverHint") }}</p>
							</div>
							<Switch id="tg-send-cover" v-model="form.sendCover" class="shrink-0" />
						</div>
						<div v-if="form.sendVideo && form.sendCover" class="flex items-start justify-between gap-3">
							<div class="flex flex-col gap-0.5">
								<Label for="tg-cover-first" class="text-xs">{{ t("telegram.coverFirst") }}</Label>
								<p class="text-[11px] text-muted-foreground">{{ t("telegram.coverFirstHint") }}</p>
							</div>
							<Switch id="tg-cover-first" v-model="form.coverFirst" class="shrink-0" />
						</div>
						<div class="flex items-start justify-between gap-3">
							<div class="flex flex-col gap-0.5">
								<Label for="tg-send-caption" class="text-xs">{{ t("telegram.sendCaption") }}</Label>
								<p class="text-[11px] text-muted-foreground">{{ t("telegram.sendCaptionHint") }}</p>
							</div>
							<Switch id="tg-send-caption" v-model="form.sendCaption" class="shrink-0" />
						</div>
						<div class="flex items-center justify-between gap-3">
							<Label for="tg-topic" class="text-xs">{{ t("telegram.topicPerModel") }}</Label>
							<Switch id="tg-topic" v-model="form.topicPerModel" />
						</div>
					</div>

					<!-- 视频 / 文字 / 封面图全关时没有内容可发 / Nothing to send when all three are off -->
					<div
						v-if="!hasPayload"
						class="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-[11px] text-destructive"
					>
						<TriangleAlert class="size-4 shrink-0 mt-0.5" />
						<span>{{ t("telegram.nothingToSend") }}</span>
					</div>

					<div class="flex items-start justify-between gap-3 rounded-md border p-3">
						<div class="flex flex-col gap-0.5">
							<Label for="tg-delete" class="text-xs">{{ t("telegram.deleteAfterSend") }}</Label>
							<p class="text-[11px] text-muted-foreground">{{ t("telegram.deleteAfterSendHint") }}</p>
						</div>
						<Switch id="tg-delete" v-model="form.deleteAfterSend" class="shrink-0" />
					</div>

					<div
						v-if="willDelete"
						class="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-[11px] text-destructive"
					>
						<TriangleAlert class="size-4 shrink-0 mt-0.5" />
						<span>{{ t("telegram.deleteWarning", { count: paths.length }) }}</span>
					</div>
				</template>

				<!-- 日志 / Log -->
				<div v-if="showLogs" class="flex flex-col gap-1">
					<Label class="text-xs text-muted-foreground">{{ t("telegram.log") }}</Label>
					<div
						class="max-h-40 overflow-y-auto rounded-md bg-muted/50 p-2 font-mono text-[10px] leading-relaxed"
					>
						<div
							v-for="(l, i) in logLines"
							:key="i"
							class="whitespace-pre-wrap break-all"
							:class="l.stream === 'stderr' ? 'text-destructive' : 'text-muted-foreground'"
						>
							{{ l.line }}
						</div>
					</div>
				</div>
			</div>

			<DialogFooter class="px-4 py-3 shrink-0 border-t gap-2 flex-row justify-end">
				<template v-if="sending">
					<Button variant="outline" class="gap-1.5" @click="cancelAll">
						<Square class="size-3.5" />
						{{ t("telegram.cancelSend") }}
					</Button>
				</template>
				<template v-else-if="allFinished">
					<Button @click="close">
						{{ t("common.close") }}
					</Button>
				</template>
				<template v-else>
					<Button variant="outline" @click="close">{{ t("common.cancel") }}</Button>
					<Button class="gap-1.5" :disabled="!canSubmit" @click="submit">
						<Loader2 v-if="loading" class="size-3.5 animate-spin" />
						<Send v-else class="size-3.5" />
						{{ t("telegram.sendNow", { count: paths.length }) }}
					</Button>
				</template>
			</DialogFooter>

			<!-- 成功汇总 / Success summary -->
			<div
				v-if="allFinished"
				class="px-4 pb-3 text-[11px] text-muted-foreground"
			>
				<span v-if="successCount > 0" class="inline-flex items-center gap-1">
					<Trash2 v-if="states.some((s) => s.deleted)" class="size-3" />
					{{ t("telegram.finishedSummary", { ok: successCount, total: paths.length }) }}
				</span>
			</div>
		</DialogContent>
	</Dialog>
</template>
