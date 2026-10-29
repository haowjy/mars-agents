
// OpenCode 2.x glue (verified against the version named in HOOK.verified).
export default {
  id: `mars.${HOOK.name}.hooks`,
  async setup(ctx: any) {
    const cwd = ctx?.location?.directory ?? process.cwd();
    const moment = (sessionId: string, rest: Partial<Moment>): Moment => ({
      cwd,
      sessionId: String(sessionId ?? ""),
      fields: {},
      ...rest,
    });
    // Registers `fn` on ctx.<group>.hook(<name>) when the hook renders one of `events`.
    const register = async (group: string, name: string, events: string[], fn: (e: any) => Promise<void>) => {
      if (!events.some((event) => HOOK.events[event])) return;
      if (typeof ctx?.[group]?.hook !== "function") {
        logOnce(`api:${group}.${name}`, `${HOOK.verified} has no ctx.${group}.hook("${name}"); ${events.join("/")} will not run`);
        return;
      }
      await ctx[group].hook(name, fn);
    };

    await register("tool", "execute.before", ["PreToolUse"], async (e) => {
      const tool = claudeTool(e.tool, e.input, cwd);
      const answer = await fire("PreToolUse", moment(e.sessionID, {
        matcher: tool.name,
        tool,
        fields: { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.id },
      }));
      if (answer.block) throw new Error(answer.block);
    });

    await register("tool", "execute.after", ["PostToolUse", "PostToolUseFailure"], async (e) => {
      const failed = e.status === "error";
      const event = failed ? "PostToolUseFailure" : "PostToolUse";
      if (!HOOK.events[event]) return;
      const tool = claudeTool(e.tool, e.input, cwd);
      const fields = failed
        ? { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.id, error: String(e.error?.message ?? e.error) }
        : { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.id, tool_response: e.result };
      const { context } = await fire(event, moment(e.sessionID, { matcher: tool.name, tool, fields }));
      if (!context) return;
      if (failed) {
        e.error.message = `${e.error.message}\n${context}`;
      } else {
        const content = e.result.content;
        const items = Array.isArray(content) ? content : typeof content === "string" ? [{ type: "text", text: content }] : [];
        e.result.content = [...items, { type: "text", text: context }];
      }
    });

    await register("session", "prompt", ["UserPromptSubmit"], async (e) => {
      const answer = await fire("UserPromptSubmit", moment(e.sessionID, { fields: { prompt: e.prompt.text } }));
      if (answer.block) throw new Error(answer.block);
      if (answer.context) e.prompt.text = `${e.prompt.text}\n\n${answer.context}`;
    });

    // The compaction hook cannot cancel: the script runs, and its answer is ignored.
    await register("session", "compaction", ["PreCompact"], async (e) => {
      await fire("PreCompact", moment(e.sessionID, {}));
    });

    await register("permission", "evaluate", ["PermissionRequest"], async (e) => {
      if (e.effect !== "ask") return;
      const tool = claudeTool(e.action, {}, cwd);
      const subject = Array.isArray(e.resources) ? e.resources[0] : undefined;
      const input = tool.name === "Bash" ? { command: subject } : typeof subject === "string" ? { file_path: subject } : {};
      const answer = await fire("PermissionRequest", moment(e.sessionID, {
        matcher: tool.name,
        fields: { tool_name: tool.name, tool_input: input },
      }));
      // A block overrides an allow, whichever handler gave which.
      if (answer.block) {
        e.effect = "deny";
        e.message = answer.block;
      } else if (answer.permission?.behavior === "allow") {
        e.effect = "allow";
      }
    });
  },
};
