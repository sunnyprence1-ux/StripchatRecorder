<!--
    后处理流水线配置页面 / Post-processing Pipeline Configuration View

    提供可视化的流水线编辑器，支持：
    - 拖拽排序节点
    - 启用/禁用节点
    - 配置每个节点的参数（字符串、数字、布尔、下拉选择）
    - 从模块选择器添加新节点
    - 自动保存（防抖 600ms）

    Provides a visual pipeline editor supporting:
    - Drag-and-drop node reordering
    - Enable/disable nodes
    - Configure per-node parameters (string, number, boolean, select)
    - Add new nodes from a module picker
    - Auto-save with 600ms debounce
-->
<script setup lang="ts">
	import { onMounted, computed, ref } from "vue";
	import {
		usePostprocessStore,
		type PipelineNode,
		type ModuleInfo,
	} from "@/stores/postprocess";
	import { useNotify } from "@/composables/useNotify";
	import { call } from "@/lib/api";
	import { Button } from "@/components/ui/button";
	import { GripVertical, ChevronUp, ChevronDown, X, Loader2, FlaskConical, KeyRound, RefreshCw } from "@lucide/vue";
	import { Switch } from "@/components/ui/switch";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import { Badge } from "@/components/ui/badge";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
	} from "@/components/ui/select";
	import {
		NumberField,
		NumberFieldContent,
		NumberFieldDecrement,
		NumberFieldIncrement,
		NumberFieldInput,
	} from "@/components/ui/number-field";
	import PipelineNodeBox from "@/components/PipelineNodeBox.vue";
	import { useI18n } from "vue-i18n";

	const store = usePostprocessStore();
	const { toast } = useNotify();
	const { t } = useI18n();

	onMounted(async () => {
		await Promise.all([store.fetchModules(), store.fetchPipeline()]);
		// 初始化模块监听器，当其他客户端更新流水线时显示提示
		// Initialize module watcher; show notification when another client updates the pipeline
		store.initModuleWatcher(() =>
			toast(t("postprocess.updatedByOther"), "info"),
		);
	});

	/** 当前可用模块 ID 集合 / Set of currently available module IDs */
	const availableModuleIds = computed(
		() => new Set(store.modules.map((m) => m.id)),
	);

	/**
	 * 判断节点对应的模块是否已不存在（模块文件被删除）。
	 * Check if a node's module is missing (module file was deleted).
	 */
	function isModuleMissing(node: PipelineNode) {
		return !availableModuleIds.value.has(node.moduleId);
	}

	/**
	 * 读取布尔参数的当前值：节点里没有保存时回落到模块声明的默认值。
	 * 这很重要——例如 `delete_after_send` 默认为 true，若只按「是否存在」判断，
	 * 开关会显示为关闭，而模块实际仍在删除数据。
	 *
	 * Read a boolean parameter's current value, falling back to the module's declared default
	 * when the node has no stored value. This matters: `delete_after_send` defaults to true, so
	 * checking mere presence would render the switch as off while the module still deletes data.
	 */
	function boolParam(
		node: PipelineNode,
		param: { key: string; default: unknown },
	): boolean {
		const v = node.params[param.key] ?? param.default;
		return v === true || v === "true";
	}

	// 拖拽排序状态 / Drag-and-drop sorting state
	/** 正在拖拽的节点索引 / Index of the node being dragged */
	const dragIndex = ref<number | null>(null);
	/** 拖拽悬停的目标节点索引 / Index of the node being dragged over */
	const dragOverIndex = ref<number | null>(null);
	/** 是否正在拖拽手柄（防止整个卡片触发拖拽）/ Whether dragging via handle (prevents card from triggering drag) */
	const isDraggingHandle = ref(false);

	/**
	 * 拖拽开始：记录源节点索引。
	 * Drag start: record source node index.
	 */
	function onDragStart(e: DragEvent, idx: number) {
		e.dataTransfer?.setData("text/plain", String(idx));
		dragIndex.value = idx;
	}

	/**
	 * 拖拽经过：更新悬停目标索引。
	 * Drag over: update hover target index.
	 */
	function onDragOver(e: DragEvent, idx: number) {
		e.preventDefault();
		dragOverIndex.value = idx;
	}

	/**
	 * 放置：执行节点位置交换。
	 * Drop: perform node position swap.
	 */
	function onDrop(e: DragEvent, idx: number) {
		e.preventDefault();
		if (dragIndex.value === null || dragIndex.value === idx) {
			dragIndex.value = null;
			dragOverIndex.value = null;
			return;
		}
		const nodes = [...store.pipeline.nodes];
		const [moved] = nodes.splice(dragIndex.value, 1);
		nodes.splice(idx, 0, moved);
		store.pipeline.nodes = nodes;
		dragIndex.value = null;
		dragOverIndex.value = null;
	}
	/**
	 * 拖拽结束：清理拖拽状态。
	 * Drag end: clean up drag state.
	 */
	function onDragEnd() {
		dragIndex.value = null;
		dragOverIndex.value = null;
		isDraggingHandle.value = false;
	}

	/** 是否显示模块选择器对话框 / Whether to show the module picker dialog */
	const showPicker = ref(false);

	/**
	 * 打开模块选择器（先刷新模块列表）。
	 * Open the module picker (refresh module list first).
	 */
	async function openPicker() {
		await store.fetchModules();
		showPicker.value = true;
	}

	/** 已在流水线中使用的模块 ID 集合（每个模块只能添加一次）/ Set of module IDs already in the pipeline (each module can only be added once) */
	const usedModuleIds = computed(
		() => new Set(store.pipeline.nodes.map((n) => n.moduleId)),
	);

	/** 尚未添加到流水线的可用模块列表 / Available modules not yet added to the pipeline */
	const availableModules = computed(() =>
		store.modules.filter((m) => !usedModuleIds.value.has(m.id)),
	);

	/**
	 * 将指定模块添加到流水线并关闭选择器。
	 * Add the specified module to the pipeline and close the picker.
	 *
	 * @param id - 模块 ID / Module ID
	 */
	function addModule(id: string) {
		store.addNode(id);
		showPicker.value = false;
	}

	/** 已启用的节点列表（用于底部统计显示）/ List of enabled nodes (for bottom stats display) */
	const enabledNodes = computed(() =>
		store.pipeline.nodes.filter((n) => n.enabled),
	);

	/** 节点列表附带对应模块信息（用于模板渲染）/ Node list with corresponding module info (for template rendering) */
	const nodesWithInfo = computed(() =>
		store.pipeline.nodes.map((node) => ({
			node,
			info: store.modules.find((m) => m.id === node.moduleId),
		})),
	);

	// ─── 模块自检 / Module self-test ────────────────────────────────────────

	/** 自检输出的一行日志 / One line of self-test output */
	interface TestLogLine {
		/** 输出流名称（stdout / stderr）/ Output stream name */
		stream: string;
		/** 行内容 / Line content */
		line: string;
	}

	/** 模块自检结果 / Module self-test result */
	interface ModuleTestResult {
		/** 是否成功 / Whether it succeeded */
		success: boolean;
		/** 结果摘要 / Result summary */
		message: string;
		/** 完整输出日志 / Full output log */
		logs: TestLogLine[];
	}

	/** 是否显示自检对话框 / Whether the self-test dialog is open */
	const showTest = ref(false);
	/** 自检是否正在运行 / Whether the self-test is running */
	const testRunning = ref(false);
	/** 自检结果（运行中为 null）/ Self-test result (null while running) */
	const testResult = ref<ModuleTestResult | null>(null);
	/** 正在测试的节点 ID（重跑时重新从 store 解析节点）/ Node ID under test (re-resolved from the store on re-run) */
	const testNodeId = ref<string | null>(null);

	/** 当前正在测试的节点及模块 / Node and module currently being tested */
	const testTarget = computed(() => {
		const node = store.pipeline.nodes.find((n) => n.nodeId === testNodeId.value);
		if (!node) return null;
		return { node, info: store.modules.find((m) => m.id === node.moduleId) };
	});

	/**
	 * 以自检模式运行节点对应的模块，并展示输出日志。
	 * 使用节点当前的参数值（未保存的修改也会生效）。
	 *
	 * Run the node's module in self-test mode and show its output log.
	 * Uses the node's current parameter values (unsaved edits included).
	 *
	 * @param node - 要测试的流水线节点 / Pipeline node to test
	 */
	async function runTest(node: PipelineNode) {
		testNodeId.value = node.nodeId;
		testResult.value = null;
		testRunning.value = true;
		showTest.value = true;

		try {
			testResult.value = await call<ModuleTestResult>("test_module", {
				moduleId: node.moduleId,
				params: { ...node.params },
			});
		} catch (e) {
			testResult.value = {
				success: false,
				message: e instanceof Error ? e.message : String(e),
				logs: [],
			};
		} finally {
			testRunning.value = false;
		}
	}

	/** 重新运行上一次的自检 / Re-run the last self-test */
	function rerunTest() {
		if (testTarget.value) runTest(testTarget.value.node);
	}

	// ─── Telegram 会话刷新 / Telegram session refresh ──────────────────────────

	/** 是否正在刷新 Telegram 会话 / Whether a Telegram relogin is in progress */
	const reloginRunning = ref(false);

	/**
	 * 刷新 Telegram 会话：重新打开已有的 session 文件、同步 auth key，无需重新输入手机号。
	 * Refresh the Telegram session: re-open the existing session file and re-sync
	 * the auth key. No phone number or code is required.
	 */
	async function telegramRelogin() {
		if (reloginRunning.value) return;
		reloginRunning.value = true;
		try {
			const res = await call<{ ok: boolean; message?: string }>("telegram_relogin");
			if (res.ok) {
				toast(t("telegram.reloginSuccess"), "success");
			} else {
				toast(res.message || t("telegram.reloginFailed"), "error");
			}
		} catch (e) {
			toast(String(e) || t("telegram.reloginFailed"), "error");
		} finally {
			reloginRunning.value = false;
		}
	}

	// ─── 交互式登录 / Interactive login ───────────────────────────────────────

	/** 登录交互结果 / Login interaction result */
	interface LoginUpdate {
		/** 登录会话 ID / Login session id */
		loginId: string;
		/** 当前状态 / Current state */
		state: string;
		/** 状态附加信息（提示、账号名、错误）/ State detail (hint, account name, error) */
		detail: string;
		/** 会话文件路径 / Session file path */
		sessionPath?: string | null;
		/** 本次交互的日志 / Logs of this interaction */
		logs: TestLogLine[];
	}

	/** 是否显示登录对话框 / Whether the login dialog is open */
	const showLogin = ref(false);
	/** 登录是否正在处理（连接、发码、校验）/ Whether a login step is in flight */
	const loginRunning = ref(false);
	/** 当前登录状态 / Current login state */
	const loginState = ref("");
	/** 状态附加信息 / State detail */
	const loginDetail = ref("");
	/** 登录会话 ID / Login session id */
	const loginId = ref<string | null>(null);
	/** 会话文件路径 / Session file path */
	const loginSessionPath = ref("");
	/** 累计日志 / Accumulated logs */
	const loginLogs = ref<TestLogLine[]>([]);
	/** 输入框内容（手机号 / 验证码 / 密码）/ Input value (phone / code / password) */
	const loginValue = ref("");
	/** 正在登录的节点 ID / Node id being logged in */
	const loginNodeId = ref<string | null>(null);

	/** 当前登录的节点及模块 / Node and module being logged in */
	const loginTarget = computed(() => {
		const node = store.pipeline.nodes.find((n) => n.nodeId === loginNodeId.value);
		if (!node) return null;
		return { node, info: store.modules.find((m) => m.id === node.moduleId) };
	});

	/**
	 * 当前登录步骤：phone（手机号）/ code（验证码）/ password（两步验证密码）/ done / error。
	 * Current login step: phone / code / password / done / error.
	 */
	const loginStep = computed(() => {
		switch (loginState.value) {
			case "":
				return "connecting";
			case "ready":
				return "phone";
			case "code_sent":
			case "invalid_code":
				return "code";
			case "password_required":
			case "invalid_password":
				return "password";
			case "done":
			case "already":
				return "done";
			default:
				return "error";
		}
	});

	/** 登录成功后的账号名 / Account name after a successful login */
	const loginAccount = computed(() => {
		if (loginStep.value !== "done") return "";
		const [name, tier] = loginDetail.value.split(":");
		return tier ? `${name} (${tier})` : name || loginDetail.value;
	});

	/** 把一次交互结果写入界面状态 / Apply one interaction result to the UI state */
	function applyLoginUpdate(update: LoginUpdate) {
		loginId.value = update.loginId || loginId.value;
		loginState.value = update.state;
		loginDetail.value = update.detail ?? "";
		if (update.sessionPath) loginSessionPath.value = update.sessionPath;
		if (update.logs?.length) loginLogs.value.push(...update.logs);
		loginValue.value = "";
	}

	/**
	 * 打开登录对话框并启动登录会话。
	 * Open the login dialog and start a login session.
	 *
	 * @param node - 要登录的流水线节点 / Pipeline node to sign in
	 * @param force - 是否强制全新登录（清除旧会话，重新输入手机号）/ Whether to force fresh login
	 */
	async function openLogin(node: PipelineNode, force = false) {
		loginNodeId.value = node.nodeId;
		loginState.value = "";
		loginDetail.value = "";
		loginId.value = null;
		loginSessionPath.value = "";
		loginLogs.value = [];
		loginValue.value = "";
		showLogin.value = true;
		loginRunning.value = true;

		try {
			applyLoginUpdate(
				await call<LoginUpdate>("login_start", {
					moduleId: node.moduleId,
					params: { ...node.params },
					force,
				}),
			);
		} catch (e) {
			loginState.value = "error";
			loginDetail.value = e instanceof Error ? e.message : String(e);
		} finally {
			loginRunning.value = false;
		}
	}

	/** 强制重新登录（清除已有会话，输入新手机号）/ Fresh re-login with a new number */
	function reloginFresh() {
		if (loginTarget.value) {
			openLogin(loginTarget.value.node, true);
		}
	}

	/**
	 * 提交当前步骤的输入（手机号 / 验证码 / 密码）。
	 * Submit the input of the current step (phone / code / password).
	 */
	async function submitLogin() {
		if (loginRunning.value || !loginId.value) return;
		const value = loginValue.value.trim();
		if (!value) return;

		const command =
			loginStep.value === "phone"
				? "phone:"
				: loginStep.value === "password"
					? "password:"
					: "code:";

		loginRunning.value = true;
		try {
			applyLoginUpdate(
				await call<LoginUpdate>("login_input", {
					loginId: loginId.value,
					value: command + value,
				}),
			);
		} catch (e) {
			loginState.value = "error";
			loginDetail.value = e instanceof Error ? e.message : String(e);
		} finally {
			loginRunning.value = false;
		}
	}

	/** 当前节点是否已经在使用用户账号（`account` 参数为 user）/ Whether the node already uses the user account */
	const loginNodeUsesUser = computed(() => {
		const node = loginTarget.value?.node;
		return !node || String(node.params.account ?? "bot") === "user";
	});

	/**
	 * 登录成功后把节点的「登录方式」切换为 user 并关闭对话框。
	 * Switch the node's login mode to `user` after a successful login and close the dialog.
	 */
	function useLoggedInAccount() {
		const node = loginTarget.value?.node;
		if (node) node.params.account = "user";
		closeLogin();
	}

	/**
	 * 关闭登录对话框；登录尚未完成时结束子进程。
	 * Close the login dialog; terminate the child process if the login is unfinished.
	 */
	async function closeLogin() {
		showLogin.value = false;
		const id = loginId.value;
		const unfinished = loginStep.value !== "done" && loginStep.value !== "error";
		loginId.value = null;
		if (id && unfinished) {
			try {
				await call("login_cancel", { loginId: id });
			} catch {
				/* 会话可能已结束，忽略 / The session may already be gone */
			}
		}
	}
</script>

<template>
	<div class="flex flex-col gap-6">
		<header class="flex flex-col gap-1 md:flex-row md:items-start md:justify-between">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("postprocess.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{ t("postprocess.description") }}
				</p>
			</div>
			<span class="text-sm text-muted-foreground md:mt-0.5 shrink-0">
				{{ store.saving ? t("postprocess.saving") : t("postprocess.saved") }}
			</span>
		</header>

		<div class="relative">
			<PipelineNodeBox :label="t('postprocess.input.label')" :description="t('postprocess.input.description')" fixed />
			<div
				v-if="store.pipeline.nodes.length > 0"
				class="absolute left-1/2 -translate-x-1/2 w-0.5 h-6 bg-border"
				style="bottom: -24px"
			/>
		</div>

		<template v-if="store.pipeline.nodes.length > 0">
			<div
				v-for="({ node, info }, idx) in nodesWithInfo"
				:key="node.nodeId"
				class="relative"
				:draggable="isDraggingHandle"
				@dragstart="onDragStart($event, idx)"
				@dragover="onDragOver($event, idx)"
				@drop="onDrop($event, idx)"
				@dragend="onDragEnd"
			>
				<div
					class="border-2 rounded-xl p-3 md:p-4 bg-card transition-opacity"
					:class="[
						!node.enabled && 'opacity-50',
						dragIndex === idx ? 'opacity-30' : '',
						isModuleMissing(node)
							? 'border-destructive/70 bg-destructive/5'
							: dragOverIndex === idx && dragIndex !== idx
								? 'border-primary'
								: 'border-transparent',
					]"
				>
					<!-- 移动端标题与操作按钮分行显示 / On mobile the title and actions take separate lines -->
					<div class="flex flex-wrap items-center gap-x-3 gap-y-2">
						<GripVertical
							class="cursor-grab text-muted-foreground select-none size-5 max-md:hidden"
							@mousedown="isDraggingHandle = true"
							@mouseup="isDraggingHandle = false"
						/>

						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2 flex-wrap">
								<span
									class="font-medium text-sm"
									:class="isModuleMissing(node) && 'text-destructive'"
									>{{ info?.name ?? node.moduleId }}</span
								>
								<Badge
									v-if="isModuleMissing(node)"
									variant="destructive"
									class="text-xs"
									>{{ t("postprocess.node.missing") }}</Badge
								>
								<Badge
									v-else-if="!node.enabled"
									variant="secondary"
									class="text-xs"
									>{{ t("postprocess.node.skipped") }}</Badge
								>
							</div>
							<p class="text-xs text-muted-foreground truncate">
								{{ info?.description ?? "" }}
							</p>
						</div>

						<div
							class="flex flex-wrap items-center justify-end gap-1.5 md:gap-2 md:shrink-0 max-md:w-full"
						>
							<Button
								v-if="info?.loginable && !isModuleMissing(node)"
								variant="outline"
								size="sm"
								class="h-7 text-xs gap-1.5"
								:title="t('postprocess.login.hint')"
								@click="openLogin(node)"
							>
								<KeyRound class="size-3.5" />
								{{ t("postprocess.login.button") }}
							</Button>
							<Button
								v-if="info?.loginable && !isModuleMissing(node) && info?.id === 'notify_telegram'"
								variant="outline"
								size="sm"
								class="h-7 text-xs gap-1.5"
								:title="t('telegram.reloginHint')"
								:disabled="reloginRunning"
								@click="telegramRelogin()"
							>
								<RefreshCw class="size-3.5" :class="reloginRunning ? 'animate-spin' : ''" />
								{{ t("telegram.relogin") }}
							</Button>
							<Button
								v-if="info?.testable && !isModuleMissing(node)"
								variant="outline"
								size="sm"
								class="h-7 text-xs gap-1.5"
								:title="t('postprocess.test.hint')"
								@click="runTest(node)"
							>
								<FlaskConical class="size-3.5" />
								{{ t("postprocess.test.button") }}
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								:disabled="idx === 0"
								@click="store.moveNode(node.nodeId, 'up')"
							><ChevronUp class="size-4" /></Button
							>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								:disabled="idx === store.pipeline.nodes.length - 1"
								@click="store.moveNode(node.nodeId, 'down')"
							><ChevronDown class="size-4" /></Button
							>

							<span class="text-xs text-muted-foreground select-none"
								>{{ t("postprocess.node.skip") }}</span
							>
							<Switch
								:model-value="!node.enabled"
								@update:model-value="node.enabled = !$event"
							/>

							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-destructive hover:text-destructive"
								@click="store.removeNode(node.nodeId)"
							><X class="size-4" /></Button
							>
						</div>
					</div>

					<div
						v-if="info?.params.length"
						class="mt-3 grid grid-cols-1 gap-x-4 gap-y-3 sm:grid-cols-2"
					>
						<template
							v-for="param in info.params"
							:key="`${node.nodeId}__${param.key}`"
						>
							<div class="flex flex-col gap-1.5">
								<Label class="text-xs">{{ param.label }}</Label>

								<Switch
									v-if="param.type === 'boolean'"
									:model-value="boolParam(node, param)"
									@update:model-value="node.params[param.key] = $event"
								/>

								<Select
									v-else-if="param.type === 'select'"
									:model-value="String(node.params[param.key] ?? param.default)"
									@update:model-value="
										node.params[param.key] = String($event ?? param.default)
									"
								>
									<SelectTrigger size="sm" class="w-full">
										<SelectValue />
									</SelectTrigger>
									<SelectContent>
										<SelectItem
											v-for="opt in param.options"
											:key="opt"
											:value="opt"
										>
											{{ opt }}
										</SelectItem>
									</SelectContent>
								</Select>

								<NumberField
									v-else-if="param.type === 'number'"
									:model-value="Number(node.params[param.key] ?? param.default)"
									@update:model-value="node.params[param.key] = $event ?? 0"
								>
									<NumberFieldContent>
										<NumberFieldDecrement />
										<NumberFieldInput />
										<NumberFieldIncrement />
									</NumberFieldContent>
								</NumberField>

								<Input
									v-else
									:model-value="String(node.params[param.key] ?? param.default)"
									class="h-8 text-sm max-md:h-10"
									@update:model-value="node.params[param.key] = String($event)"
								/>
							</div>
						</template>
					</div>
				</div>

				<div
					v-if="idx < store.pipeline.nodes.length - 1"
					class="absolute left-1/2 -translate-x-1/2 w-0.5 h-6 bg-border"
					style="bottom: -24px"
				/>
			</div>
		</template>

		<div class="flex flex-col items-center gap-2">
			<div
				v-if="store.pipeline.nodes.length === 0"
				class="text-sm text-muted-foreground"
			>
				{{ t("postprocess.empty") }}
			</div>

			<Button variant="outline" size="sm" @click="openPicker">
				{{ t("postprocess.addModule") }}
			</Button>
		</div>

		<Dialog :open="showPicker" @update:open="showPicker = $event">
			<DialogContent class="max-w-sm dialog-max-h overflow-y-auto">
				<DialogHeader>
					<DialogTitle>{{ t("postprocess.picker.title") }}</DialogTitle>
					<DialogDescription class="sr-only">{{ t("postprocess.picker.description") }}</DialogDescription>
				</DialogHeader>
				<div class="flex flex-col gap-1 mt-1">
					<div
						v-if="availableModules.length === 0"
						class="text-sm text-muted-foreground px-2 py-4 text-center"
					>
						<template v-if="store.modules.length === 0">
							{{ t("postprocess.picker.noModules") }}<br />
							<span class="text-xs"
								>{{ t("postprocess.picker.noModulesHint") }}</span
							>
						</template>
						<template v-else> {{ t("postprocess.picker.allAdded") }} </template>
					</div>
					<Button
						v-for="mod in availableModules"
						:key="mod.id"
						variant="ghost"
						class="flex flex-col items-start px-3 py-2.5 rounded-lg text-left h-auto w-full whitespace-normal"
						@click="addModule(mod.id)"
					>
						<span class="text-sm font-medium">{{ mod.name }}</span>
						<span class="text-xs text-muted-foreground">{{
							mod.description
						}}</span>
					</Button>
				</div>
			</DialogContent>
		</Dialog>

		<Dialog
			:open="showLogin"
			@update:open="
				(value: boolean) => {
					if (!value) closeLogin();
				}
			"
		>
			<DialogContent class="max-w-lg dialog-max-h overflow-y-auto">
				<DialogHeader>
					<DialogTitle>
						{{
							t("postprocess.login.title", {
								name:
									loginTarget?.info?.name ??
									loginTarget?.node.moduleId ??
									"",
							})
						}}
					</DialogTitle>
					<DialogDescription>
						{{ t("postprocess.login.description") }}
					</DialogDescription>
				</DialogHeader>

				<div class="flex flex-col gap-3">
					<!-- 连接中 / Connecting -->
					<div
						v-if="loginRunning && loginStep === 'connecting'"
						class="flex items-center gap-2 text-sm text-muted-foreground py-6 justify-center"
					>
						<Loader2 class="size-4 animate-spin" />
						{{ t("postprocess.login.connecting") }}
					</div>

					<!-- 手机号 / Phone number -->
					<template v-else-if="loginStep === 'phone'">
						<div class="flex flex-col gap-1.5">
							<Label for="login-phone" class="text-xs">{{
								t("postprocess.login.phoneLabel")
							}}</Label>
							<Input
								id="login-phone"
								v-model="loginValue"
								type="tel"
								class="h-9 text-sm"
								:placeholder="t('postprocess.login.phonePlaceholder')"
								:disabled="loginRunning"
								@keyup.enter="submitLogin"
							/>
						</div>
						<div class="flex justify-end">
							<Button
								size="sm"
								:disabled="loginRunning || !loginValue.trim()"
								@click="submitLogin"
							>
								<Loader2
									v-if="loginRunning"
									class="size-3.5 animate-spin"
								/>
								{{ t("postprocess.login.sendCode") }}
							</Button>
						</div>
					</template>

					<!-- 验证码 / Login code -->
					<template v-else-if="loginStep === 'code'">
						<p class="text-sm text-muted-foreground">
							{{ t("postprocess.login.codeSent") }}
						</p>
						<p
							v-if="loginStep === 'code' && loginState === 'invalid_code'"
							class="text-sm text-destructive"
						>
							{{ t("postprocess.login.invalidCode") }}
						</p>
						<div class="flex flex-col gap-1.5">
							<Label for="login-code" class="text-xs">{{
								t("postprocess.login.codeLabel")
							}}</Label>
							<Input
								id="login-code"
								v-model="loginValue"
								inputmode="numeric"
								autocomplete="one-time-code"
								class="h-9 text-sm"
								placeholder="12345"
								:disabled="loginRunning"
								@keyup.enter="submitLogin"
							/>
						</div>
						<div class="flex justify-end">
							<Button
								size="sm"
								:disabled="loginRunning || !loginValue.trim()"
								@click="submitLogin"
							>
								<Loader2
									v-if="loginRunning"
									class="size-3.5 animate-spin"
								/>
								{{ t("postprocess.login.verify") }}
							</Button>
						</div>
					</template>

					<!-- 两步验证密码 / Two-factor password -->
					<template v-else-if="loginStep === 'password'">
						<p class="text-sm text-muted-foreground">
							{{
								loginDetail
									? t("postprocess.login.passwordHint", {
											hint: loginDetail,
										})
									: t("postprocess.login.passwordHintNone")
							}}
						</p>
						<p
							v-if="loginState === 'invalid_password'"
							class="text-sm text-destructive"
						>
							{{ t("postprocess.login.invalidPassword") }}
						</p>
						<div class="flex flex-col gap-1.5">
							<Label for="login-password" class="text-xs">{{
								t("postprocess.login.passwordLabel")
							}}</Label>
							<Input
								id="login-password"
								v-model="loginValue"
								type="password"
								class="h-9 text-sm"
								:disabled="loginRunning"
								@keyup.enter="submitLogin"
							/>
						</div>
						<div class="flex justify-end">
							<Button
								size="sm"
								:disabled="loginRunning || !loginValue.trim()"
								@click="submitLogin"
							>
								<Loader2
									v-if="loginRunning"
									class="size-3.5 animate-spin"
								/>
								{{ t("postprocess.login.verify") }}
							</Button>
						</div>
					</template>

					<!-- 成功 / Success -->
					<template v-else-if="loginStep === 'done'">
						<div
							class="flex flex-col gap-1 rounded-lg border border-emerald-500/40 bg-emerald-500/5 p-3 text-sm"
						>
							<span class="font-medium">
								{{
									loginState === "already"
										? t("postprocess.login.already", {
												name: loginAccount,
											})
										: t("postprocess.login.done", {
												name: loginAccount,
											})
								}}
							</span>
							<span class="text-muted-foreground text-xs">
								{{ t("postprocess.login.doneHint") }}
							</span>
							<span
								v-if="loginSessionPath"
								class="text-muted-foreground text-xs font-mono break-all"
							>
								{{
									t("postprocess.login.sessionPath", {
										path: loginSessionPath,
									})
								}}
							</span>
							<div class="flex flex-wrap items-center gap-2 mt-2">
								<Button
									v-if="loginNodeUsesUser"
									size="sm"
									class="self-start"
									@click="useLoggedInAccount"
								>
									{{ t("postprocess.login.useAccount") }}
								</Button>
								<span
									v-else
									class="text-muted-foreground text-xs"
									>✓ {{ t("postprocess.login.accountActive") }}</span
								>
								<Button
									variant="outline"
									size="sm"
									class="text-xs gap-1.5"
									:disabled="loginRunning"
									@click="reloginFresh"
								>
									<RefreshCw class="size-3.5" :class="loginRunning ? 'animate-spin' : ''" />
									{{ t("postprocess.login.reloginNewNumber") }}
								</Button>
							</div>
						</div>
					</template>

					<!-- 失败 / Failure -->
					<template v-else>
						<div
							class="flex flex-col gap-1 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm"
						>
							<span class="font-medium">{{
								t("postprocess.login.failed")
							}}</span>
							<span class="text-muted-foreground break-all">{{
								loginDetail
							}}</span>
						</div>
					</template>

					<div
						v-if="loginLogs.length"
						class="max-h-40 overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-xs leading-relaxed"
					>
						<div
							v-for="(log, i) in loginLogs"
							:key="i"
							class="whitespace-pre-wrap break-all"
							:class="
								log.stream === 'stderr'
									? 'text-destructive'
									: 'text-foreground'
							"
						>
							{{ log.line }}
						</div>
					</div>
				</div>

				<!-- 移动端底部按钮固定在对话框底部 / Footer buttons stick to the bottom on mobile -->
				<div class="flex justify-end gap-2 sticky bottom-0 z-10 bg-background pt-2">
					<Button variant="ghost" size="sm" @click="closeLogin">
						{{ t("common.close") }}
					</Button>
					<Button
						v-if="loginStep === 'error' && loginTarget"
						variant="outline"
						size="sm"
						:disabled="loginRunning"
						@click="openLogin(loginTarget.node)"
					>
						{{ t("postprocess.login.retry") }}
					</Button>
				</div>
			</DialogContent>
		</Dialog>

		<Dialog :open="showTest" @update:open="showTest = $event">
			<DialogContent class="max-w-2xl dialog-max-h overflow-y-auto">
				<DialogHeader>
					<DialogTitle>
						{{
							t("postprocess.test.title", {
								name:
									testTarget?.info?.name ??
									testTarget?.node.moduleId ??
									"",
							})
						}}
					</DialogTitle>
					<DialogDescription>
						{{ t("postprocess.test.description") }}
					</DialogDescription>
				</DialogHeader>

				<div
					v-if="testRunning"
					class="flex items-center gap-2 text-sm text-muted-foreground py-6 justify-center"
				>
					<Loader2 class="size-4 animate-spin" />
					{{ t("postprocess.test.running") }}
				</div>

				<template v-else-if="testResult">
					<div
						class="flex items-start gap-2 rounded-lg border p-3 text-sm"
						:class="
							testResult.success
								? 'border-emerald-500/40 bg-emerald-500/5'
								: 'border-destructive/40 bg-destructive/5'
						"
					>
						<span class="font-medium shrink-0">
							{{
								testResult.success
									? t("postprocess.test.success")
									: t("postprocess.test.failed")
							}}
						</span>
						<span class="text-muted-foreground break-all">{{
							testResult.message
						}}</span>
					</div>

					<div
						v-if="testResult.logs.length"
						class="max-h-72 overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-xs leading-relaxed"
					>
						<div
							v-for="(log, i) in testResult.logs"
							:key="i"
							class="whitespace-pre-wrap break-all"
							:class="
								log.stream === 'stderr'
									? 'text-destructive'
									: 'text-foreground'
							"
						>
							{{ log.line }}
						</div>
					</div>
				</template>

				<!-- 移动端底部按钮固定在对话框底部 / Footer buttons stick to the bottom on mobile -->
				<div class="flex justify-end gap-2 sticky bottom-0 z-10 bg-background pt-2">
					<Button variant="ghost" size="sm" @click="showTest = false">
						{{ t("common.close") }}
					</Button>
					<Button
						variant="outline"
						size="sm"
						:disabled="testRunning"
						@click="rerunTest"
					>
						{{ t("postprocess.test.rerun") }}
					</Button>
				</div>
			</DialogContent>
		</Dialog>

		<div
			v-if="store.pipeline.nodes.length > 0"
			class="text-xs text-muted-foreground text-center"
		>
			{{ t("postprocess.stats", { enabled: enabledNodes.length, total: store.pipeline.nodes.length }) }}
		</div>
	</div>
</template>
