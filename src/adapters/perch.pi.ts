/**
 * perch — managed by `perch install pi`. Edits here are overwritten.
 *
 * Reports pi's session lifecycle to `perch hook pi` on stdin. The child is
 * detached and its output discarded so a slow or missing perch can never block
 * or fail the agent.
 */

import { spawn } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

type Payload = {
	event: string;
	session_id: string | null;
	cwd: string;
	last_message: string | null;
};

function report(event: string, ctx: any, last_message: string | null): void {
	let payload: Payload;
	try {
		payload = {
			event,
			session_id: ctx?.sessionManager?.getSessionId?.() ?? null,
			cwd: process.cwd(),
			last_message,
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

export default function (pi: ExtensionAPI) {
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
