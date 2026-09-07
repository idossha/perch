/**
 * perch — managed by `perch install pi`. Edits here are overwritten.
 *
 * Two jobs:
 *
 * 1. Reports pi's session lifecycle to `perch hook pi` on stdin. The child is
 *    detached and its output discarded so a slow or missing perch can never
 *    block or fail the agent.
 * 2. Gives pi's model an `ask_user` tool — pi has no question tool of its own.
 *    The tool sends its questions to `perch hook pi --ask`, which pops perch's
 *    answer form up on every attached tmux client and waits; the answers come
 *    back on stdout and are returned to the model. If perch hands the question
 *    back (the popup was closed, the deadline passed, perch is not installed),
 *    the tool falls back to pi's own select and input dialogs, so the user is
 *    always asked somewhere.
 */

import { spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

type Payload = {
	event: string;
	session_id: string | null;
	cwd: string;
	last_message: string | null;
	model: string | null;
	effort: string | null;
};

function report(event: string, ctx: any, last_message: string | null): void {
	let payload: Payload;
	try {
		payload = {
			event,
			session_id: ctx?.sessionManager?.getSessionId?.() ?? null,
			cwd: process.cwd(),
			last_message,
			model: ctx?.model?.id ?? null,
			effort: ctx?.thinkingLevel ?? null,
		};
	} catch {
		return;
	}
	try {
		const child = spawn("perch", ["hook", "pi"], {
			detached: true,
			stdio: ["pipe", "ignore", "ignore"],
		});
		child.on("error", () => {});
		child.stdin?.on("error", () => {});
		child.stdin?.end(JSON.stringify(payload));
		child.unref();
	} catch {
		// perch is not installed, or spawning failed: never disturb the agent.
	}
}

/** The text of the last assistant message in `event.messages`, if there is one. */
function lastAssistantText(event: any): string | null {
	const messages = event?.messages;
	if (!Array.isArray(messages)) return null;
	for (let i = messages.length - 1; i >= 0; i--) {
		const m = messages[i];
		if (m?.role !== "assistant") continue;
		if (typeof m.content === "string") return m.content;
		if (Array.isArray(m.content)) {
			const text = m.content
				.filter((p: any) => typeof p?.text === "string")
				.map((p: any) => p.text)
				.join("");
			if (text) return text;
		}
	}
	return null;
}

type QuestionSpec = {
	question: string;
	header?: string;
	options: { label: string; description?: string }[];
	multiSelect?: boolean;
};

type Answers = Record<string, string>;

/**
 * Ask through perch: run `perch hook pi --ask`, hand it the questions, and
 * wait for its answer. Resolves to the answers, or to null when perch handed
 * the question back or could not be run at all.
 */
function askPerch(payload: unknown, signal: AbortSignal | undefined): Promise<Answers | null> {
	return new Promise((resolve) => {
		let child: ReturnType<typeof spawn>;
		try {
			child = spawn("perch", ["hook", "pi", "--ask"], {
				stdio: ["pipe", "pipe", "ignore"],
			});
		} catch {
			resolve(null);
			return;
		}
		let out = "";
		let settled = false;
		const done = (value: Answers | null) => {
			if (settled) return;
			settled = true;
			resolve(value);
		};
		child.on("error", () => done(null));
		child.stdout?.on("data", (chunk: Buffer) => {
			out += chunk.toString();
		});
		child.on("close", () => {
			try {
				const parsed = JSON.parse(out);
				const answers = parsed?.answers;
				if (answers && typeof answers === "object" && Object.keys(answers).length > 0) {
					done(answers as Answers);
					return;
				}
			} catch {
				// Nothing usable on stdout: perch handed the question back.
			}
			done(null);
		});
		signal?.addEventListener("abort", () => {
			try {
				child.kill();
			} catch {}
			done(null);
		});
		child.stdin?.on("error", () => {});
		child.stdin?.end(JSON.stringify(payload));
	});
}

/** Ask in pi's own dialogs: one select per question, `Other…` for free text. */
async function askInPi(questions: QuestionSpec[], ctx: any): Promise<Answers> {
	const answers: Answers = {};
	for (const [i, q] of questions.entries()) {
		const labels = q.options.map((o) => o.label);
		const tag = questions.length > 1 ? `Question ${i + 1}/${questions.length}${q.header ? ` · ${q.header}` : ""}: ` : "";
		const title = `${tag}${q.question}`;
		if (q.multiSelect) {
			const picked: string[] = [];
			for (;;) {
				const remaining = labels.filter((l) => !picked.includes(l));
				const choice = await ctx.ui.select(title, [...remaining, "Done"]);
				if (choice === undefined || choice === "Done") break;
				picked.push(choice);
			}
			if (picked.length) answers[q.question] = picked.join(", ");
			continue;
		}
		const choice = await ctx.ui.select(title, [...labels, "Other…"]);
		if (choice === undefined) continue;
		if (choice === "Other…") {
			const text = await ctx.ui.input(title, "Type your answer");
			if (text && text.trim()) answers[q.question] = text.trim();
			continue;
		}
		answers[q.question] = choice;
	}
	return answers;
}

const QuestionSchema = Type.Object({
	question: Type.String({ description: "One self-contained sentence. It is also the key the answer comes back under, so keep it unique within the call." }),
	header: Type.Optional(Type.String({ description: "Two or three words, at most twelve characters: the tab label." })),
	options: Type.Array(
		Type.Object({
			label: Type.String({ description: "Short choice text." }),
			description: Type.Optional(Type.String({ description: "One line on what choosing it leads to." })),
		}),
		{ description: "Two to four options, the recommended one first, marked (Recommended). No Other or Skip: the form provides free text." },
	),
	multiSelect: Type.Optional(Type.Boolean({ description: "Several answers may hold at once." })),
});

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "ask_user",
		label: "Ask the user",
		description:
			"Ask the user one to four multiple-choice questions and wait for their answers. Use it whenever you need a decision, a clarification, a preference or an approval you cannot resolve from the code or the conversation — never ask such things in plain chat. Put every question you have right now into ONE call (up to four), not one call per question: they are shown together as tabs. The questions appear in front of the user wherever they are (perch pops it up on every tmux client) and the answers come back as a map from question text to the chosen label; multi-select answers are comma-separated; a question the user left unanswered is absent — take your recommended option and say so.",
		promptSnippet: "ask_user: ask the user a multiple-choice question when you need a decision; never ask in chat",
		parameters: Type.Object({
			questions: Type.Array(QuestionSchema, { minItems: 1, maxItems: 4 }),
		}),
		executionMode: "sequential",
		async execute(toolCallId: string, params: any, signal: AbortSignal | undefined, _onUpdate: any, ctx: any) {
			const questions: QuestionSpec[] = params.questions ?? [];
			const payload = {
				event: "ask_user",
				session_id: ctx?.sessionManager?.getSessionId?.() ?? null,
				cwd: process.cwd(),
				tool_use_id: toolCallId,
				tool_input: { questions },
			};
			let answers = await askPerch(payload, signal);
			if (answers === null) {
				answers = await askInPi(questions, ctx);
			}
			const lines = Object.entries(answers).map(([q, a]) => `${q} → ${a}`);
			const text = lines.length
				? `User answered:\n${lines.join("\n")}`
				: "The user did not answer. Take your recommended option and say so.";
			return { content: [{ type: "text", text }], details: { answers } };
		},
	});

	pi.on("session_start", async (_event: any, ctx: any) => {
		report("session_start", ctx, null);
		return undefined;
	});
	pi.on("agent_start", async (_event: any, ctx: any) => {
		report("agent_start", ctx, null);
		return undefined;
	});
	pi.on("agent_end", async (event: any, ctx: any) => {
		report("agent_end", ctx, lastAssistantText(event));
		return undefined;
	});
	pi.on("session_shutdown", async (_event: any, ctx: any) => {
		report("session_shutdown", ctx, null);
		return undefined;
	});
}
