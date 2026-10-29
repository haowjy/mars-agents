
// Pi 1.4 glue (verified against the version named in HOOK.verified).
const START_SOURCES: any = { startup: "startup", resume: "resume", new: "clear" };
const END_REASONS: any = { quit: "prompt_input_exit", new: "clear", resume: "resume" };
const MAX_STOP_ROUNDS = 8;

export default function (pi: any) {
  if (typeof pi?.on !== "function") {
    logOnce("api:on", `${HOOK.verified} has no pi.on; no event will run`);
    return;
  }
  const on = (events: string[], name: string, fn: (e: any, ctx: any) => any) => {
    if (events.some((event) => HOOK.events[event])) pi.on(name, fn);
  };
  const moment = (ctx: any, rest: Partial<Moment>): Moment => {
    let sessionId = "";
    try {
      sessionId = String(ctx.sessionManager.getSessionId());
    } catch {
      logOnce("api:session", `${HOOK.verified} has no ctx.sessionManager.getSessionId(); session_id is empty`);
    }
    return { cwd: ctx?.cwd ?? process.cwd(), sessionId, fields: {}, ...rest };
  };
  const hidden = (content: string) => ({ customType: `mars-${HOOK.name}`, content, display: false });
  const textOf = (content: any): string =>
    Array.isArray(content) ? content.map((item: any) => (item?.type === "text" ? item.text : "")).join("") : String(content ?? "");

  on(["PreToolUse"], "tool_call", async (e, ctx) => {
    const tool = claudeTool(e.toolName, e.input, ctx?.cwd ?? process.cwd());
    const answer = await fire("PreToolUse", moment(ctx, {
      matcher: tool.name,
      tool,
      fields: { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.toolCallId },
    }));
    if (answer.block) return { block: true, reason: answer.block };
  });

  on(["PostToolUse", "PostToolUseFailure"], "tool_result", async (e, ctx) => {
    const event = e.isError ? "PostToolUseFailure" : "PostToolUse";
    if (!HOOK.events[event]) return;
    const tool = claudeTool(e.toolName, e.input, ctx?.cwd ?? process.cwd());
    const fields = e.isError
      ? { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.toolCallId, error: textOf(e.content) }
      : { tool_name: tool.name, tool_input: tool.input, tool_use_id: e.toolCallId, tool_response: { content: e.content, details: e.details } };
    const { context } = await fire(event, moment(ctx, { matcher: tool.name, tool, fields }));
    if (context) return { content: [...(e.content ?? []), { type: "text", text: context }] };
  });

  // The script runs once, in `input`; its context waits for before_agent_start.
  let promptContext: string | undefined;
  on(["UserPromptSubmit"], "input", async (e, ctx) => {
    const answer = await fire("UserPromptSubmit", moment(ctx, { fields: { prompt: e.text } }));
    if (answer.block) {
      ctx?.ui?.notify?.(answer.block, "error");
      return { action: "handled" };
    }
    promptContext = answer.context;
  });
  on(["UserPromptSubmit"], "before_agent_start", () => {
    const content = promptContext;
    promptContext = undefined;
    if (content) return { message: hidden(content) };
  });

  // Pi emits session_start twice when a session is replaced (seen live); once per session id.
  let startedSession = "";
  on(["SessionStart"], "session_start", async (e, ctx) => {
    const current = moment(ctx, {});
    if (current.sessionId !== "" && current.sessionId === startedSession) return;
    startedSession = current.sessionId;
    const source = START_SOURCES[e.reason] ?? e.reason;
    const { context } = await fire("SessionStart", moment(ctx, { matcher: source, fields: { source } }));
    if (context) pi.sendMessage(hidden(context));
  });

  // `stop_hook_active` is true on re-entry, capped like Claude's loop guard.
  let stopRounds = 0;
  on(["Stop"], "agent_before_settle", async (_e, ctx) => {
    const answer = await fire("Stop", moment(ctx, { fields: { stop_hook_active: stopRounds > 0 } }));
    if (answer.block && stopRounds < MAX_STOP_ROUNDS) {
      stopRounds += 1;
      return { continue: true, entries: [{ type: "custom_message", ...hidden(answer.block) }] };
    }
    stopRounds = 0;
  });
  on(["Stop"], "agent_settled", () => {
    stopRounds = 0;
  });

  on(["PreCompact"], "session_before_compact", async (e, ctx) => {
    const trigger = e.reason === "manual" ? "manual" : "auto";
    const answer = await fire("PreCompact", moment(ctx, { matcher: trigger, fields: { trigger } }));
    if (answer.block) {
      ctx?.ui?.notify?.(answer.block, "error");
      return { cancel: true };
    }
  });
  on(["PostCompact"], "session_compact", async (e, ctx) => {
    const trigger = e.reason === "manual" ? "manual" : "auto";
    await fire("PostCompact", moment(ctx, { matcher: trigger, fields: { trigger } }));
  });

  on(["SessionEnd"], "session_shutdown", async (e, ctx) => {
    const reason = END_REASONS[e.reason] ?? "other";
    await fire("SessionEnd", moment(ctx, { matcher: reason, fields: { reason } }));
  });
}
