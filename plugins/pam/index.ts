import { completeSimple } from "@earendil-works/pi-ai/compat";
import type { Model, ThinkingLevel, UserMessage } from "@earendil-works/pi-ai";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
	ExtensionContext,
	Theme,
} from "@earendil-works/pi-coding-agent";
import {
	fuzzyFilter,
	Input,
	matchesKey,
	truncateToWidth,
	type TUI,
	wrapTextWithAnsi,
} from "@earendil-works/pi-tui";
import { Type } from "typebox";

const PROVIDER = "alexandria";
const STATE_ENTRY = "pam-mode-state";
const DEFAULT_MODE: ModeName = "medium";
const SETTINGS_PATH = fileURLToPath(new URL("./settings.json", import.meta.url));

const MODE_NAMES = ["low", "medium", "high", "ultra"] as const;
const THINKING_LEVELS = ["minimal", "low", "medium", "high", "xhigh", "max"] as const;
type ModeName = (typeof MODE_NAMES)[number];
type ModeColor = "thinkingLow" | "thinkingMedium" | "thinkingHigh" | "thinkingMax";
type ModeRole = "agent" | "oracle";
type SettingsTarget = { mode: ModeName; role: ModeRole };
type DialAction =
	| { type: "apply"; mode: ModeName }
	| { type: "settings"; mode: ModeName };

type ModelChoice = {
	ids: readonly string[];
	display: string;
	thinking: ThinkingLevel;
};

type PamMode = {
	name: ModeName;
	color: ModeColor;
	agent: ModelChoice;
	oracle: ModelChoice;
	description: string;
	instructions: string;
};

const DEFAULT_MODES: Record<ModeName, PamMode> = {
	low: {
		name: "low",
		color: "thinkingLow",
		agent: {
			ids: ["alex/openrouter/z-ai/glm-5.2"],
			display: "GLM-5.2",
			thinking: "medium",
		},
		oracle: {
			ids: ["alex/gpt-5.6-sol", "alex/openrouter/openai/gpt-5.6-sol"],
			display: "GPT-5.6 Sol",
			thinking: "high",
		},
		description: "Fast, low-cost mode for small, well-defined tasks",
		instructions: [
			"Pam mode: low.",
			"The task should already be well-defined. Work directly and keep exploration proportional.",
			"Prefer a small, focused implementation and targeted verification. Ask only when a missing decision materially changes the result.",
		].join(" "),
	},
	medium: {
		name: "medium",
		color: "thinkingMedium",
		agent: {
			ids: ["alex/gpt-5.6-sol", "alex/openrouter/openai/gpt-5.6-sol"],
			display: "GPT-5.6 Sol",
			thinking: "medium",
		},
		oracle: {
			ids: ["alex/gpt-5.6-sol", "alex/openrouter/openai/gpt-5.6-sol"],
			display: "GPT-5.6 Sol",
			thinking: "high",
		},
		description: "The balanced default for messy, multi-part work",
		instructions: [
			"Pam mode: medium.",
			"Resolve ordinary ambiguity, inspect enough context to catch unstated steps, and keep the work easy to steer.",
			"Deliver a complete result with verification appropriate to the risk.",
		].join(" "),
	},
	high: {
		name: "high",
		color: "thinkingHigh",
		agent: {
			ids: ["alex/gpt-5.6-sol", "alex/openrouter/openai/gpt-5.6-sol"],
			display: "GPT-5.6 Sol",
			thinking: "xhigh",
		},
		oracle: {
			ids: ["alex/claude-fable-5", "alex/openrouter/anthropic/claude-fable-5"],
			display: "Claude Fable 5",
			thinking: "high",
		},
		description: "High capability for difficult work where subtle misses are expensive",
		instructions: [
			"Pam mode: high.",
			"Treat subtle misses as expensive. Trace cross-cutting behavior, concurrency, compatibility, and edge cases before committing to a change.",
			"Use the Pam oracle for an independent review when a consequential choice or non-obvious failure mode merits it.",
		].join(" "),
	},
	ultra: {
		name: "ultra",
		color: "thinkingMax",
		agent: {
			ids: ["alex/claude-fable-5", "alex/openrouter/anthropic/claude-fable-5"],
			display: "Claude Fable 5",
			thinking: "high",
		},
		oracle: {
			ids: ["alex/gpt-5.6-sol", "alex/openrouter/openai/gpt-5.6-sol"],
			display: "GPT-5.6 Sol",
			thinking: "high",
		},
		description: "The most capable mode for hard, open-ended tasks",
		instructions: [
			"Pam mode: ultra.",
			"The outcome may be clear while the path is not. Discover the system deeply, make architecture and migration decisions explicitly, and carry multi-file work through verification.",
			"Use the Pam oracle as a genuine second opinion on important plans, assumptions, and the final result.",
		].join(" "),
	},
};

type ChoiceSettings = {
	model?: unknown;
	fallbacks?: unknown;
	thinking?: unknown;
};

type SettingsLoadResult = {
	modes: Record<ModeName, PamMode>;
	warnings: string[];
};

function loadModeSettings(): SettingsLoadResult {
	const modes = cloneModes(DEFAULT_MODES);
	const warnings: string[] = [];
	let parsed: unknown;
	try {
		parsed = JSON.parse(readFileSync(SETTINGS_PATH, "utf8"));
	} catch (error) {
		return {
			modes,
			warnings: [`Could not read ${SETTINGS_PATH}; using Pam defaults: ${errorText(error)}`],
		};
	}

	if (!isRecord(parsed)) {
		return { modes, warnings: [`${SETTINGS_PATH} must contain a JSON object; using Pam defaults`] };
	}

	for (const modeName of MODE_NAMES) {
		const modeSettings = parsed[modeName];
		if (modeSettings === undefined) continue;
		if (!isRecord(modeSettings)) {
			warnings.push(`${modeName} must be an object; using its defaults`);
			continue;
		}
		for (const role of ["agent", "oracle"] as const) {
			const choiceSettings = modeSettings[role];
			if (choiceSettings === undefined) continue;
			if (!isRecord(choiceSettings)) {
				warnings.push(`${modeName}.${role} must be an object; using its default`);
				continue;
			}
			applyChoiceSettings(modes[modeName][role], choiceSettings, `${modeName}.${role}`, warnings);
		}
	}

	return { modes, warnings };
}

function applyChoiceSettings(
	choice: ModelChoice,
	settings: ChoiceSettings,
	label: string,
	warnings: string[],
): void {
	let ids = [...choice.ids];
	if (settings.model !== undefined) {
		if (isAlexModel(settings.model)) {
			ids = [settings.model];
		} else {
			warnings.push(`${label}.model must be a non-empty alex/* model id; using its default`);
		}
	}
	if (settings.fallbacks !== undefined) {
		if (!Array.isArray(settings.fallbacks)) {
			warnings.push(`${label}.fallbacks must be an array; ignoring it`);
		} else {
			const fallbacks = settings.fallbacks.filter(isAlexModel);
			if (fallbacks.length !== settings.fallbacks.length) {
				warnings.push(`${label}.fallbacks contains invalid entries; only alex/* strings were kept`);
			}
			ids.push(...fallbacks);
		}
	}
	choice.ids = [...new Set(ids)];
	choice.display = modelDisplay(choice.ids[0]);

	if (settings.thinking !== undefined) {
		if (isThinkingLevel(settings.thinking)) {
			choice.thinking = settings.thinking;
		} else {
			warnings.push(`${label}.thinking must be one of ${THINKING_LEVELS.join(", ")}; using its default`);
		}
	}
}

function cloneModes(source: Record<ModeName, PamMode>): Record<ModeName, PamMode> {
	const clone = (mode: PamMode): PamMode => ({
		...mode,
		agent: { ...mode.agent, ids: [...mode.agent.ids] },
		oracle: { ...mode.oracle, ids: [...mode.oracle.ids] },
	});
	return {
		low: clone(source.low),
		medium: clone(source.medium),
		high: clone(source.high),
		ultra: clone(source.ultra),
	};
}

function saveModeSettings(modes: Record<ModeName, PamMode>): void {
	const settings = Object.fromEntries(
		MODE_NAMES.map((name) => [
			name,
			{
				agent: choiceToSettings(modes[name].agent),
				oracle: choiceToSettings(modes[name].oracle),
			},
		]),
	);
	writeFileSync(SETTINGS_PATH, `${JSON.stringify(settings, null, 2)}\n`, "utf8");
}

function choiceToSettings(choice: ModelChoice): Record<string, unknown> {
	return {
		model: choice.ids[0],
		...(choice.ids.length > 1 ? { fallbacks: choice.ids.slice(1) } : {}),
		thinking: choice.thinking,
	};
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAlexModel(value: unknown): value is string {
	return typeof value === "string" && value.startsWith("alex/") && value.length > "alex/".length;
}

function isThinkingLevel(value: unknown): value is ThinkingLevel {
	return typeof value === "string" && (THINKING_LEVELS as readonly string[]).includes(value);
}

function modelDisplay(id: string): string {
	const bare = id.split("/").at(-1) ?? id;
	const knownNames: Record<string, string> = {
		"glm-5.2": "GLM-5.2",
		"gpt-5.6-sol": "GPT-5.6 Sol",
		"gpt-5.6-terra": "GPT-5.6 Terra",
		"gpt-5.6-luna": "GPT-5.6 Luna",
		"gpt-5.5": "GPT-5.5",
		"gpt-5.5-codex": "GPT-5.5 Codex",
		"claude-fable-5": "Claude Fable 5",
		"claude-opus-4-8": "Claude Opus 4.8",
		"claude-sonnet-5": "Claude Sonnet 5",
		"gemini-2.5-flash": "Gemini 2.5 Flash",
		"grok-code-fast-1": "Grok Code Fast 1",
	};
	if (knownNames[bare]) return knownNames[bare];
	return bare
		.split("-")
		.map((part) => {
			if (["gpt", "glm", "ai"].includes(part.toLowerCase())) return part.toUpperCase();
			return part ? `${part[0].toUpperCase()}${part.slice(1)}` : part;
		})
		.join(" ");
}

function canonicalModelName(model: Model<any>): string {
	const name = model.name.trim();
	const bareId = model.id.split("/").at(-1) ?? model.id;
	return name && name !== model.id && name !== bareId && !name.includes("/") ? name : modelDisplay(model.id);
}

function refreshModeDisplays(modes: Record<ModeName, PamMode>, ctx: ExtensionContext): void {
	for (const modeName of MODE_NAMES) {
		for (const role of ["agent", "oracle"] as const) {
			const choice = modes[modeName][role];
			const model = findModel(ctx, choice.ids);
			choice.display = model ? canonicalModelName(model) : modelDisplay(choice.ids[0]);
		}
	}
}

type StoredMode = { mode: ModeName | null };

const OracleParams = Type.Object({
	question: Type.String({
		description: "The concrete question, plan, or proposed change for the oracle to review",
	}),
	context: Type.Optional(
		Type.String({
			description: "Relevant code, constraints, evidence, or reasoning needed for an informed second opinion",
		}),
	),
});

export default function pam(pi: ExtensionAPI) {
	let modes = cloneModes(DEFAULT_MODES);
	let activeMode: ModeName | undefined;
	let applyingMode = false;
	let dialOpen = false;
	let settingsOpen = false;
	let removeTerminalShortcut: (() => void) | undefined;

	const updateStatus = (ctx: ExtensionContext) => {
		if (!activeMode) {
			ctx.ui.setStatus("pam", undefined);
			return;
		}
		const mode = modes[activeMode];
		ctx.ui.setStatus("pam", ctx.ui.theme.fg(mode.color, `pam:${mode.name}`));
	};

	const persistMode = (mode: ModeName | null) => {
		pi.appendEntry<StoredMode>(STATE_ENTRY, { mode });
	};

	const restoreMode = (ctx: ExtensionContext): ModeName | null | undefined => {
		const entry = ctx.sessionManager
			.getBranch()
			.filter(
				(item: { type: string; customType?: string }) =>
					item.type === "custom" && item.customType === STATE_ENTRY,
			)
			.pop() as { data?: StoredMode } | undefined;
		return entry?.data?.mode;
	};

	const clearMode = (ctx: ExtensionContext, persist = true) => {
		if (!activeMode) return;
		activeMode = undefined;
		if (persist) persistMode(null);
		updateStatus(ctx);
	};

	const applyMode = async (
		name: ModeName,
		ctx: ExtensionContext,
		options: { persist?: boolean; notify?: boolean } = {},
	): Promise<boolean> => {
		const mode = modes[name];
		const model = findModel(ctx, mode.agent.ids);
		if (!model) {
			if (ctx.hasUI) {
				ctx.ui.notify(missingModelMessage(name, "agent", mode.agent.ids), "error");
			}
			return false;
		}

		applyingMode = true;
		try {
			const selected = await pi.setModel(withReasoning(model));
			if (!selected) {
				if (ctx.hasUI) ctx.ui.notify(`Pam could not authenticate ${model.id}`, "error");
				return false;
			}
			pi.setThinkingLevel(mode.agent.thinking);
			activeMode = name;
			if (options.persist !== false) persistMode(name);
			updateStatus(ctx);
			if (options.notify !== false && ctx.hasUI) {
				ctx.ui.notify(
					`Pam ${name}: ${mode.agent.display} ${mode.agent.thinking} · oracle ${mode.oracle.display} ${mode.oracle.thinking}`,
					"info",
				);
			}
			return true;
		} catch (error) {
			if (ctx.hasUI) ctx.ui.notify(`Pam could not switch modes: ${errorText(error)}`, "error");
			return false;
		} finally {
			applyingMode = false;
		}
	};

	let showSettings: (ctx: ExtensionContext) => Promise<void>;

	const showDial = async (ctx: ExtensionContext): Promise<void> => {
		if (ctx.mode !== "tui") {
			if (ctx.hasUI) ctx.ui.notify("Use /pam low|medium|high|ultra outside the TUI", "warning");
			return;
		}
		if (!ctx.isIdle()) {
			ctx.ui.notify("Pam can turn the dial after the current agent turn finishes", "warning");
			return;
		}
		if (dialOpen) return;

		let initial = activeMode ?? inferMode(modes, ctx.model, pi.getThinkingLevel()) ?? DEFAULT_MODE;
		while (true) {
			let action: DialAction | undefined;
			dialOpen = true;
			try {
				action = await ctx.ui.custom<DialAction | undefined>(
					(tui, theme, _keybindings, done) => new PamDial(tui, theme, modes, initial, done),
					{
						overlay: true,
						overlayOptions: {
							anchor: "center",
							width: "90%",
							minWidth: 48,
							maxHeight: 18,
							margin: 1,
						},
					},
				);
			} finally {
				dialOpen = false;
			}

			if (!action) return;
			if (action.type === "apply") {
				await applyMode(action.mode, ctx);
				return;
			}

			initial = action.mode;
			await showSettings(ctx);
		}
	};

	showSettings = async (ctx: ExtensionContext): Promise<void> => {
		if (ctx.mode !== "tui") {
			if (ctx.hasUI) {
				ctx.ui.notify(`${settingsSummary(modes)}\nEdit ${SETTINGS_PATH}, then run /reload.`, "info");
			}
			return;
		}
		if (!ctx.isIdle()) {
			ctx.ui.notify("Pam settings can open after the current agent turn finishes", "warning");
			return;
		}
		if (settingsOpen) return;

		ctx.modelRegistry.refresh();
		refreshModeDisplays(modes, ctx);
		const registryError = ctx.modelRegistry.getError();
		if (registryError) ctx.ui.notify(`Pi model catalog: ${registryError}`, "warning");
		const availableModels = ctx.modelRegistry
			.getAvailable()
			.filter((model) => model.provider === PROVIDER && model.id.startsWith("alex/"))
			.sort((left, right) => left.id.localeCompare(right.id));
		if (availableModels.length === 0) {
			ctx.ui.notify(
				"Pi has no available Alex alex/* models. Run 'alex connect pi', then restart Pi.",
				"error",
			);
			return;
		}

		settingsOpen = true;
		try {
			let lastTarget: SettingsTarget | undefined;
			while (true) {
				const target = await ctx.ui.custom<SettingsTarget | undefined>(
					(tui, theme, _keybindings, done) =>
						new PamSettingsMenu(tui, theme, modes, lastTarget, done),
					{
						overlay: true,
						overlayOptions: {
							anchor: "center",
							width: "90%",
							minWidth: 54,
							maxHeight: 20,
							margin: 1,
						},
					},
				);
				if (!target) return;
				lastTarget = target;

				const choice = modes[target.mode][target.role];
				const selectedModel = await ctx.ui.custom<Model<any> | undefined>(
					(tui, theme, _keybindings, done) =>
						new PamModelPicker(tui, theme, availableModels, choice.ids[0], target, done),
					{
						overlay: true,
						overlayOptions: {
							anchor: "center",
							width: "90%",
							minWidth: 54,
							maxHeight: 20,
							margin: 1,
						},
					},
				);
				if (!selectedModel) continue;

				const previous = { ...choice, ids: [...choice.ids] };
				choice.ids = [selectedModel.id];
				choice.display = canonicalModelName(selectedModel);
				try {
					saveModeSettings(modes);
				} catch (error) {
					modes[target.mode][target.role] = previous;
					ctx.ui.notify(`Could not save Pam settings: ${errorText(error)}`, "error");
					continue;
				}

				if (activeMode === target.mode && target.role === "agent") {
					await applyMode(target.mode, ctx, { persist: false, notify: false });
				}
				ctx.ui.notify(`Pam ${target.mode} ${target.role}: ${selectedModel.id}`, "info");
			}
		} finally {
			settingsOpen = false;
		}
	};

	const bindTerminalShortcut = (ctx: ExtensionContext) => {
		removeTerminalShortcut?.();
		removeTerminalShortcut = undefined;
		if (ctx.mode !== "tui") return;
		removeTerminalShortcut = ctx.ui.onTerminalInput((data) => {
			if (!matchesKey(data, "ctrl+s")) return;
			if (!dialOpen && !settingsOpen) void showDial(ctx);
			return { consume: true };
		});
	};

	pi.registerCommand("pam", {
		description: "Open Pam's capability dial or show its model settings",
		getArgumentCompletions: (prefix) => {
			const choices = [...MODE_NAMES, "settings", "off"].filter((name) => name.startsWith(prefix));
			return choices.length > 0 ? choices.map((name) => ({ value: name, label: name })) : null;
		},
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			await ctx.waitForIdle();
			const requested = args.trim().toLowerCase();
			if (!requested) {
				await showDial(ctx);
				return;
			}
			if (requested === "settings") {
				await showSettings(ctx);
				return;
			}
			if (requested === "off") {
				clearMode(ctx);
				ctx.ui.notify("Pam mode disabled; the current model was left unchanged", "info");
				return;
			}
			if (!isModeName(requested)) {
				ctx.ui.notify(
					`Unknown Pam mode '${requested}'. Use low, medium, high, ultra, settings, or off.`,
					"error",
				);
				return;
			}
			await applyMode(requested, ctx);
		},
	});

	pi.registerTool({
		name: "pam_oracle",
		label: "Pam Oracle",
		description:
			"Ask Pam's mode-specific oracle model for an independent second opinion. Use it for consequential plans, subtle bugs, architecture choices, or review when the active Pam instructions call for it. Include enough context for the oracle to reason independently.",
		promptGuidelines: [
			"Use pam_oracle when an independent model review could catch an expensive mistake.",
			"State a concrete question and include the relevant evidence or proposed approach in context.",
			"Treat the response as advice: reconcile it with repository evidence before acting.",
		],
		parameters: OracleParams,
		async execute(_toolCallId, params, signal, onUpdate, ctx) {
			if (!activeMode) throw new Error("Pam mode is not active. Select one with /pam first.");

			const mode = modes[activeMode];
			const model = findModel(ctx, mode.oracle.ids);
			if (!model) throw new Error(missingModelMessage(activeMode, "oracle", mode.oracle.ids));
			const oracleModel = withReasoning(model);
			const auth = await ctx.modelRegistry.getApiKeyAndHeaders(oracleModel);
			if (!auth.ok) throw new Error(auth.error);
			if (!auth.apiKey) throw new Error(`No API key is available for ${oracleModel.id}`);

			onUpdate?.({
				content: [{ type: "text", text: `Consulting ${mode.oracle.display} (${mode.oracle.thinking})…` }],
				details: { mode: activeMode, model: oracleModel.id, thinking: mode.oracle.thinking },
			});

			const userMessage: UserMessage = {
				role: "user",
				content: [
					{
						type: "text",
						text: params.context
							? `Question:\n${params.question}\n\nContext:\n${params.context}`
							: params.question,
					},
				],
				timestamp: Date.now(),
			};

			const response = await completeSimple(
				oracleModel,
				{
					systemPrompt: oracleSystemPrompt(activeMode),
					messages: [userMessage],
				},
				{
					apiKey: auth.apiKey,
					headers: auth.headers,
					env: auth.env,
					reasoning: mode.oracle.thinking,
					signal,
				},
			);

			if (response.stopReason === "error" || response.stopReason === "aborted") {
				throw new Error(response.errorMessage ?? `Oracle stopped: ${response.stopReason}`);
			}
			const text = response.content
				.filter((part): part is Extract<(typeof response.content)[number], { type: "text" }> => part.type === "text")
				.map((part) => part.text)
				.join("\n")
				.trim();
			if (!text) throw new Error(`Oracle returned no text (${response.stopReason})`);

			return {
				content: [{ type: "text", text }],
				details: {
					mode: activeMode,
					model: oracleModel.id,
					thinking: mode.oracle.thinking,
					stopReason: response.stopReason,
				},
			};
		},
	});

	pi.on("before_agent_start", (event) => {
		if (!activeMode) return;
		return {
			systemPrompt: `${event.systemPrompt}\n\n${modes[activeMode].instructions}`,
		};
	});

	pi.on("model_select", (_event, ctx) => {
		if (!applyingMode) clearMode(ctx);
	});

	pi.on("thinking_level_select", (_event, ctx) => {
		if (!applyingMode) clearMode(ctx);
	});

	pi.on("session_start", async (_event, ctx) => {
		const loaded = loadModeSettings();
		modes = loaded.modes;
		refreshModeDisplays(modes, ctx);
		for (const warning of loaded.warnings) ctx.ui.notify(`Pam settings: ${warning}`, "warning");
		bindTerminalShortcut(ctx);
		const restored = restoreMode(ctx);
		if (restored && isModeName(restored)) {
			await applyMode(restored, ctx, { persist: false, notify: false });
			return;
		}
		if (restored === null) {
			activeMode = undefined;
			updateStatus(ctx);
			return;
		}
		await applyMode(DEFAULT_MODE, ctx, { notify: false });
	});

	pi.on("session_tree", (_event, ctx) => {
		const restored = restoreMode(ctx);
		activeMode = restored && isModeName(restored) ? restored : undefined;
		updateStatus(ctx);
	});

	pi.on("session_shutdown", () => {
		removeTerminalShortcut?.();
		removeTerminalShortcut = undefined;
	});
}

class PamSettingsMenu {
	private readonly targets: SettingsTarget[] = MODE_NAMES.flatMap((mode) => [
		{ mode, role: "agent" },
		{ mode, role: "oracle" },
	]);
	private selected = 0;

	constructor(
		private readonly tui: TUI,
		private readonly theme: Theme,
		private readonly modes: Record<ModeName, PamMode>,
		initial: SettingsTarget | undefined,
		private readonly done: (target: SettingsTarget | undefined) => void,
	) {
		if (initial) {
			const index = this.targets.findIndex(
				(target) => target.mode === initial.mode && target.role === initial.role,
			);
			if (index >= 0) this.selected = index;
		}
	}

	handleInput(data: string): void {
		if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
			this.done(undefined);
			return;
		}
		if (matchesKey(data, "enter") || matchesKey(data, "return")) {
			this.done(this.targets[this.selected]);
			return;
		}
		if (matchesKey(data, "up")) {
			this.selected = this.selected === 0 ? this.targets.length - 1 : this.selected - 1;
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "down")) {
			this.selected = this.selected === this.targets.length - 1 ? 0 : this.selected + 1;
			this.tui.requestRender();
		}
	}

	render(width: number): string[] {
		const th = this.theme;
		const innerWidth = Math.max(1, width - 2);
		const contentWidth = Math.max(1, innerWidth - 4);
		const lines = [th.fg("border", `╭${"─".repeat(innerWidth)}╮`)];
		const row = (content = "") => framedRow(th, content, innerWidth, 2);

		lines.push(row(th.bold("Pam model settings")));
		lines.push(row(th.fg("muted", "Choose a tier and role to change its primary model")));
		lines.push(row(""));
		for (let index = 0; index < this.targets.length; index++) {
			const target = this.targets[index];
			const choice = this.modes[target.mode][target.role];
			const prefix = index === this.selected ? th.fg("accent", "› ") : "  ";
			const mode = modeText(th, this.modes[target.mode], target.mode.padEnd(7));
			const role = th.fg("text", target.role.padEnd(8));
			const name = index === this.selected ? th.fg("accent", choice.display) : th.fg("text", choice.display);
			const id = th.fg("dim", `  ${choice.ids[0]}`);
			lines.push(row(truncateToWidth(`${prefix}${mode}${role}${name}${id}`, contentWidth)));
		}
		lines.push(row(""));
		lines.push(row(th.fg("dim", "↑↓ choose  ·  enter pick model  ·  esc close")));
		lines.push(th.fg("border", `╰${"─".repeat(innerWidth)}╯`));
		return lines;
	}

	invalidate(): void {}
}

class PamModelPicker {
	private readonly input = new Input();
	private filtered: Model<any>[];
	private selected = 0;
	private _focused = false;

	get focused(): boolean {
		return this._focused;
	}

	set focused(value: boolean) {
		this._focused = value;
		this.input.focused = value;
	}

	constructor(
		private readonly tui: TUI,
		private readonly theme: Theme,
		private readonly models: Model<any>[],
		private readonly currentId: string,
		private readonly target: SettingsTarget,
		private readonly done: (model: Model<any> | undefined) => void,
	) {
		this.filtered = models;
		const current = models.findIndex((model) => model.id === currentId);
		this.selected = current >= 0 ? current : 0;
	}

	handleInput(data: string): void {
		if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
			this.done(undefined);
			return;
		}
		if (matchesKey(data, "enter") || matchesKey(data, "return")) {
			const selected = this.filtered[this.selected];
			if (selected) this.done(selected);
			return;
		}
		if (matchesKey(data, "up")) {
			if (this.filtered.length > 0) {
				this.selected = this.selected === 0 ? this.filtered.length - 1 : this.selected - 1;
			}
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "down")) {
			if (this.filtered.length > 0) {
				this.selected = this.selected === this.filtered.length - 1 ? 0 : this.selected + 1;
			}
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
			const direction = matchesKey(data, "pageUp") ? -1 : 1;
			this.selected = Math.max(0, Math.min(this.filtered.length - 1, this.selected + direction * 8));
			this.tui.requestRender();
			return;
		}

		this.input.handleInput(data);
		const query = this.input.getValue().trim();
		this.filtered = query
			? fuzzyFilter(this.models, query, (model) => `${model.id} ${model.name}`)
			: this.models;
		this.selected = 0;
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const th = this.theme;
		const innerWidth = Math.max(1, width - 2);
		const contentWidth = Math.max(1, innerWidth - 4);
		const lines = [th.fg("border", `╭${"─".repeat(innerWidth)}╮`)];
		const row = (content = "") => framedRow(th, content, innerWidth, 2);

		lines.push(
			row(
				`${th.bold("Pick model for ")}${modeText(th, DEFAULT_MODES[this.target.mode], this.target.mode)} ${this.target.role}`,
			),
		);
		const inputLine = this.input.render(Math.max(1, contentWidth - 8))[0] ?? "";
		lines.push(row(`${th.fg("muted", "Search: ")}${inputLine}`));
		lines.push(row(""));

		const maxVisible = 8;
		const start = Math.max(
			0,
			Math.min(this.selected - Math.floor(maxVisible / 2), Math.max(0, this.filtered.length - maxVisible)),
		);
		const end = Math.min(start + maxVisible, this.filtered.length);
		if (this.filtered.length === 0) {
			lines.push(row(th.fg("warning", "No matching Alex models")));
		} else {
			for (let index = start; index < end; index++) {
				const model = this.filtered[index];
				const isSelected = index === this.selected;
				const prefix = isSelected ? th.fg("accent", "› ") : "  ";
				const check = model.id === this.currentId ? th.fg("success", " ✓") : "";
				const friendlyName = canonicalModelName(model);
				const name = isSelected ? th.fg("accent", friendlyName) : th.fg("text", friendlyName);
				const id = th.fg("dim", `  ${model.id}`);
				lines.push(row(truncateToWidth(`${prefix}${name}${check}${id}`, contentWidth)));
			}
			lines.push(row(th.fg("dim", `${this.selected + 1}/${this.filtered.length} available alex/* models`)));
		}
		lines.push(row(""));
		lines.push(row(th.fg("dim", "type to search  ·  ↑↓ choose  ·  enter save  ·  esc back")));
		lines.push(th.fg("border", `╰${"─".repeat(innerWidth)}╯`));
		return lines;
	}

	invalidate(): void {
		this.input.invalidate();
	}
}

class PamDial {
	private selected: number;
	private phase = 0;
	private interval: ReturnType<typeof setInterval> | undefined;

	constructor(
		private readonly tui: TUI,
		private readonly theme: Theme,
		private readonly modes: Record<ModeName, PamMode>,
		initial: ModeName,
		private readonly done: (action: DialAction | undefined) => void,
	) {
		this.selected = MODE_NAMES.indexOf(initial);
		this.interval = setInterval(() => {
			this.phase++;
			this.tui.requestRender();
		}, 90);
	}

	handleInput(data: string): void {
		if (matchesKey(data, "ctrl+c")) {
			this.finish(undefined);
			return;
		}
		if (matchesKey(data, "escape")) {
			this.finish(undefined);
			return;
		}
		if (matchesKey(data, "enter") || matchesKey(data, "return")) {
			this.finish({ type: "apply", mode: MODE_NAMES[this.selected] });
			return;
		}
		if (data === "s" || data === "S") {
			this.finish({ type: "settings", mode: MODE_NAMES[this.selected] });
			return;
		}
		if (matchesKey(data, "left") || matchesKey(data, "up")) {
			this.selected = Math.max(0, this.selected - 1);
			this.phase = 0;
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "right") || matchesKey(data, "down")) {
			this.selected = Math.min(MODE_NAMES.length - 1, this.selected + 1);
			this.phase = 0;
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "home")) {
			this.selected = 0;
			this.phase = 0;
			this.tui.requestRender();
			return;
		}
		if (matchesKey(data, "end")) {
			this.selected = MODE_NAMES.length - 1;
			this.phase = 0;
			this.tui.requestRender();
		}
	}

	private finish(action: DialAction | undefined): void {
		this.dispose();
		this.done(action);
	}

	render(width: number): string[] {
		const th = this.theme;
		const innerWidth = Math.max(1, width - 2);
		const sidePadding = innerWidth >= 54 ? 3 : 1;
		const contentWidth = Math.max(1, innerWidth - sidePadding * 2);
		const selectedMode = this.modes[MODE_NAMES[this.selected]];
		const sheenPhase = Math.floor(this.phase / 2);
		const agentTextLength = choiceDisplay(selectedMode.agent).length;
		const oracleTextLength = choiceDisplay(selectedMode.oracle).length;
		const sheenGap = 6;
		const agentSheenStart = 0;
		const oracleSheenStart = agentSheenStart + agentTextLength + sheenGap;
		const descriptionSheenStart = oracleSheenStart + oracleTextLength + sheenGap;
		const sheenCycleLength = descriptionSheenStart + selectedMode.description.length + sheenGap;
		const lines: string[] = [];
		let rowIndex = 0;

		const row = (content = "") => {
			const padded = `${" ".repeat(sidePadding)}${content}`;
			const left = electricSide(th, selectedMode, this.phase, rowIndex, 0);
			const right = electricSide(th, selectedMode, this.phase, rowIndex, 5);
			rowIndex++;
			return left + truncateToWidth(padded, innerWidth, "", true) + right;
		};

		lines.push(electricBorder(th, selectedMode, innerWidth, this.phase, false));
		lines.push(row(""));
		lines.push(row(renderDots(th, selectedMode, this.selected, contentWidth)));
		lines.push(row(renderLabels(th, this.modes, this.selected, contentWidth)));
		lines.push(row(""));
		lines.push(
			row(
				detailLine(
					th,
					"Agent:",
					selectedMode.agent,
					selectedMode,
					sheenPhase,
					agentSheenStart,
					sheenCycleLength,
				),
			),
		);
		lines.push(
			row(
				detailLine(
					th,
					"Oracle:",
					selectedMode.oracle,
					selectedMode,
					sheenPhase,
					oracleSheenStart,
					sheenCycleLength,
				),
			),
		);
		lines.push(row(""));
		const glowingDescription = sheenText(
			th,
			selectedMode,
			selectedMode.description,
			sheenPhase,
			descriptionSheenStart,
			sheenCycleLength,
			"muted",
		);
		for (const descriptionLine of wrapTextWithAnsi(glowingDescription, contentWidth)) {
			lines.push(row(descriptionLine));
		}
		lines.push(row(""));
		lines.push(row(th.fg("dim", "←→ choose  ·  enter apply  ·  s settings  ·  esc cancel")));
		lines.push(electricBorder(th, selectedMode, innerWidth, this.phase + 11, true));
		return lines;
	}

	invalidate(): void {}

	dispose(): void {
		if (this.interval) {
			clearInterval(this.interval);
			this.interval = undefined;
		}
	}
}

function framedRow(theme: Theme, content: string, innerWidth: number, sidePadding: number): string {
	const padded = `${" ".repeat(sidePadding)}${content}`;
	return theme.fg("border", "│") + truncateToWidth(padded, innerWidth, "", true) + theme.fg("border", "│");
}

function electricBorder(
	theme: Theme,
	mode: PamMode,
	innerWidth: number,
	phase: number,
	bottom: boolean,
): string {
	const rippleRadius = mode.name === "medium" ? 3 : mode.name === "high" ? 4 : 2;
	const hotspot = bottom
		? innerWidth - 1 - positiveMod(phase, innerWidth)
		: positiveMod(phase, innerWidth);
	let rail = "";
	for (let index = 0; index < innerWidth; index++) {
		const directDistance = Math.abs(index - hotspot);
		const distance = Math.min(directDistance, innerWidth - directDistance);
		if (distance === 0) {
			rail += theme.fg("text", theme.bold("━"));
		} else if (distance <= rippleRadius) {
			rail += modeText(theme, mode, theme.bold("╍"));
		} else {
			rail += energeticModeText(theme, mode, "─");
		}
	}
	const left = modeText(theme, mode, theme.bold(bottom ? "╰" : "╭"));
	const right = modeText(theme, mode, theme.bold(bottom ? "╯" : "╮"));
	return `${left}${rail}${right}`;
}

function electricSide(theme: Theme, mode: PamMode, phase: number, row: number, offset: number): string {
	const period = mode.name === "medium" ? 12 : mode.name === "high" ? 10 : 14;
	const pulse = positiveMod(phase - row * 2 + offset, period);
	if (pulse === 0) return theme.fg("text", theme.bold("┃"));
	if (pulse <= 2 || pulse >= period - 1) return modeText(theme, mode, theme.bold("╏"));
	return energeticModeText(theme, mode, "│");
}

function renderDots(theme: Theme, mode: PamMode, selected: number, width: number): string {
	const dotCount = Math.max(8, Math.floor((width + 1) / 2));
	const activeCount = 1 + Math.round((selected / (MODE_NAMES.length - 1)) * (dotCount - 1));
	return Array.from({ length: dotCount }, (_, index) => {
		const dot =
			index === activeCount - 1
				? theme.fg("text", theme.bold("●"))
				: index >= activeCount
				? theme.fg("dim", "•")
				: energeticModeText(theme, mode, "•");
		return index === dotCount - 1 ? dot : `${dot} `;
	}).join("");
}

function renderLabels(
	theme: Theme,
	modes: Record<ModeName, PamMode>,
	selected: number,
	width: number,
): string {
	const positions = [0, Math.floor(width / 3), Math.floor((2 * width) / 3), width - MODE_NAMES[3].length];
	let cursor = 0;
	let line = "";
	for (let index = 0; index < MODE_NAMES.length; index++) {
		const name = MODE_NAMES[index];
		const position = Math.max(cursor, positions[index]);
		line += " ".repeat(Math.max(0, position - cursor));
		line +=
			index === selected
				? modeText(theme, modes[name], theme.bold(name))
				: theme.fg("muted", name);
		cursor = position + name.length;
	}
	return line;
}

function detailLine(
	theme: Theme,
	label: string,
	choice: ModelChoice,
	mode: PamMode,
	phase: number,
	start: number,
	cycleLength: number,
): string {
	const value = choiceDisplay(choice);
	return `${theme.fg("text", label.padEnd(9))}${sheenText(theme, mode, value, phase, start, cycleLength, "muted")}`;
}

function choiceDisplay(choice: ModelChoice): string {
	return `${choice.display} ${choice.thinking}`;
}

function sheenText(
	theme: Theme,
	mode: PamMode,
	text: string,
	phase: number,
	start: number,
	cycleLength: number,
	base: "mode" | "muted",
): string {
	const hotspot = positiveMod(phase, cycleLength) - start;
	return [...text]
		.map((character, index) => {
			if (character === " ") return character;
			const distance = Math.abs(index - hotspot);
			if (distance === 0) return theme.fg("text", theme.bold(character));
			if (distance === 1) return modeText(theme, mode, theme.bold(character));
			return base === "mode" ? modeText(theme, mode, theme.bold(character)) : theme.fg("muted", character);
		})
		.join("");
}

function positiveMod(value: number, divisor: number): number {
	return ((value % divisor) + divisor) % divisor;
}

function modeText(theme: Theme, mode: PamMode, text: string): string {
	return theme.fg(mode.color, text);
}

function energeticModeText(theme: Theme, mode: PamMode, text: string): string {
	const styled = mode.name === "medium" || mode.name === "high" ? theme.bold(text) : text;
	return modeText(theme, mode, styled);
}

function findModel(ctx: ExtensionContext, ids: readonly string[]): Model<any> | undefined {
	for (const id of ids) {
		const model = ctx.modelRegistry.find(PROVIDER, id);
		if (model) return model;
	}
	return undefined;
}

function withReasoning(model: Model<any>): Model<any> {
	return model.reasoning ? model : { ...model, reasoning: true };
}

function inferMode(
	modes: Record<ModeName, PamMode>,
	model: Model<any> | undefined,
	thinking: string,
): ModeName | undefined {
	if (!model || model.provider !== PROVIDER) return undefined;
	return MODE_NAMES.find((name) => {
		const mode = modes[name];
		return mode.agent.ids.includes(model.id) && mode.agent.thinking === thinking;
	});
}

function isModeName(value: string): value is ModeName {
	return (MODE_NAMES as readonly string[]).includes(value);
}

function missingModelMessage(mode: ModeName, role: ModeRole, ids: readonly string[]): string {
	return `Pam ${mode} ${role} could not find ${ids.join(" or ")} in Pi's '${PROVIDER}' provider. Check ${SETTINGS_PATH}, or refresh the catalog with 'alex connect pi', then restart Pi.`;
}

function settingsSummary(modes: Record<ModeName, PamMode>): string {
	return MODE_NAMES.map((name) => {
		const mode = modes[name];
		return `${name}: agent ${mode.agent.ids[0]} (${mode.agent.thinking}), oracle ${mode.oracle.ids[0]} (${mode.oracle.thinking})`;
	}).join("\n");
}

function oracleSystemPrompt(mode: ModeName): string {
	return [
		`You are the independent oracle for Pam's ${mode} mode.`,
		"Review the supplied question and context on their merits.",
		"Look especially for false assumptions, missed edge cases, simpler alternatives, and verification gaps.",
		"Do not use tools or pretend to inspect anything not included in the context.",
		"Give a decisive, concise recommendation and clearly label uncertainty.",
	].join(" ");
}

function errorText(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}
