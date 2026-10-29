import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";

// The part shared by both harnesses: Claude's tool names and inputs, matcher and `if` rules,
// the command runner, and the answer mapping. The harness glue below it only registers
// native callbacks and turns an Outcome into the harness's own block / context / keep-going.
// Imports no harness package, so it loads on any version.

const LABEL = `mars hook ${HOOK.name} (${HOOK.harness})`;
const DEFAULT_TIMEOUT_SECONDS = 600;
// Events whose answer can block the action: only these honour `onFailure = "block"`.
const BLOCKING = new Set(["PreToolUse", "UserPromptSubmit", "Stop", "PreCompact", "PermissionRequest"]);
const CONTEXT = new Set(["PostToolUse", "PostToolUseFailure", "UserPromptSubmit", "SessionStart"]);
const CLAUDE_TOOLS = new Map<string, string>([
  ["shell", "Bash"],
  ["bash", "Bash"],
  ["read", "Read"],
  ["write", "Write"],
  ["edit", "Edit"],
]);

const logged = new Set<string>();
function logOnce(key: string, message: string): void {
  if (logged.has(key)) return;
  logged.add(key);
  console.error(`[${LABEL}] ${message}`);
}

// What the script answered, in terms the glue maps to the harness.
type Outcome = {
  block?: string;
  context?: string;
  permission?: { behavior: "allow" | "deny"; message?: string };
};

function claudeTool(native: string, input: any, cwd: string): { name: string; input: any } {
  const name = CLAUDE_TOOLS.get(native);
  if (!name) return { name: native, input: input ?? {} };
  if (name === "Bash") return { name, input: { command: input?.command ?? "" } };
  const path = input?.path ?? input?.filePath ?? input?.file_path;
  const out: any = {};
  if (typeof path === "string") out.file_path = isAbsolute(path) ? path : resolve(cwd, path);
  if (name === "Write" && input?.content !== undefined) out.content = input.content;
  if (name === "Edit") {
    const one = Array.isArray(input?.edits) && input.edits.length === 1 ? input.edits[0] : undefined;
    const oldText = input?.oldString ?? one?.oldText;
    const newText = input?.newString ?? one?.newText;
    if (typeof oldText === "string") out.old_string = oldText;
    if (typeof newText === "string") out.new_string = newText;
  }
  return { name, input: out };
}

// Claude's matcher rule: exact (`|` or `,` list) for plain names, else an unanchored regex.
function matcherAccepts(pattern: string | null | undefined, value: string): boolean {
  if (pattern == null || pattern === "" || pattern === "*") return true;
  if (/^[A-Za-z0-9_ ,|-]+$/.test(pattern)) {
    return pattern.split(/[|,]/).some((name) => name.trim() === value);
  }
  try {
    return new RegExp(pattern).test(value);
  } catch {
    logOnce(`regex:${pattern}`, `matcher \`${pattern}\` is not a valid regular expression; it matches nothing`);
    return false;
  }
}

// A superset of Claude's `if`: the tool must match, and every literal part of the glob must
// appear in the command or path, so a script never runs less often than under Claude.
function conditionHolds(rule: string, tool: string, input: any): boolean {
  const open = rule.indexOf("(");
  if (!matcherAccepts((open < 0 ? rule : rule.slice(0, open)).trim(), tool)) return false;
  if (open < 0 || !rule.endsWith(")")) return true;
  const subject = tool === "Bash" ? input?.command : input?.file_path;
  if (typeof subject !== "string") return true;
  const haystack = tool === "Bash" ? subject : subject.replace(/\\/g, "/");
  return rule
    .slice(open + 1, -1)
    .replace(/:\*$/, "*")
    .split("*")
    .map((part) => part.replace(/^(~|\.\/|\/(?=\/))/, ""))
    .filter((part) => part !== "")
    .every((part) => haystack.includes(part));
}

let gitBash: string | null | undefined;
// Windows: the Git install's bash, never System32\bash.exe, which is WSL's and cannot
// resolve Windows paths (D22).
function findGitBash(): string | null {
  if (gitBash !== undefined) return gitBash;
  const env = process.env;
  const found: string[] = [];
  if (env.CLAUDE_CODE_GIT_BASH_PATH) found.push(env.CLAUDE_CODE_GIT_BASH_PATH);
  for (const base of [env.ProgramFiles, env["ProgramFiles(x86)"], env.LOCALAPPDATA && join(env.LOCALAPPDATA, "Programs")]) {
    if (base) found.push(join(base, "Git", "bin", "bash.exe"));
  }
  const where = spawnSync("where", ["git"], { encoding: "utf8", windowsHide: true });
  for (const git of String(where.stdout ?? "").split(/\r?\n/).filter(Boolean)) {
    found.push(join(dirname(git), "..", "bin", "bash.exe"), join(dirname(git), "..", "..", "bin", "bash.exe"));
  }
  gitBash = found.find((path) => existsSync(path) && !/[\\/]system32[\\/]/i.test(path)) ?? null;
  return gitBash;
}

type Run = { ok: boolean; status: number | null; stdout: string; stderr: string; why: string };

function runCommand(handler: any, stdin: string, cwd: string): Promise<Run> {
  return new Promise((settle) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let done = false;
    const finish = (run: Run) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      settle(run);
    };
    const fail = (why: string, stdout = "", stderr = "") => finish({ ok: false, status: null, stdout, stderr, why });
    const windows = process.platform === "win32";
    const shell = windows ? findGitBash() : "sh";
    if (!shell) return fail("Git Bash was not found, so the command cannot run");
    const command = windows && handler.commandWindows ? handler.commandWindows : handler.command;
    let child: any;
    try {
      child = spawn(shell, ["-c", command], {
        cwd,
        env: { ...process.env, PWD: cwd },
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true,
        detached: !windows,
      });
    } catch (error: any) {
      return fail(String(error?.message ?? error));
    }
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => (stderr += chunk));
    child.stdin.on("error", () => {});
    child.on("error", (error: any) => fail(String(error?.message ?? error), stdout, stderr));
    child.on("close", (status: number | null, signal: string | null) =>
      finish({ ok: status !== null, status, stdout, stderr, why: signal ? `killed by ${signal}` : `exit ${status}` }),
    );
    const seconds = handler.timeout ?? DEFAULT_TIMEOUT_SECONDS;
    timer = setTimeout(() => {
      try {
        if (windows) child.kill();
        else process.kill(-child.pid, "SIGKILL");
      } catch {}
      fail(`timed out after ${seconds}s`, stdout, stderr);
    }, seconds * 1000);
    child.stdin.end(stdin);
  });
}

// The portable answers (design § What a script sees and answers); any other field is ignored.
function readAnswer(event: string, stdout: string): Outcome | null {
  const text = stdout.trim();
  if (!text.startsWith("{")) return null;
  let json: any;
  try {
    json = JSON.parse(text);
  } catch {
    return null;
  }
  const specific = json?.hookSpecificOutput ?? {};
  const answer: Outcome = {};
  if (event === "PreToolUse" && specific.permissionDecision === "deny") {
    answer.block = specific.permissionDecisionReason || `Blocked by hook ${HOOK.name}`;
  }
  if ((event === "UserPromptSubmit" || event === "Stop") && json.decision === "block") {
    answer.block = json.reason || `Blocked by hook ${HOOK.name}`;
  }
  if (CONTEXT.has(event) && typeof specific.additionalContext === "string" && specific.additionalContext !== "") {
    answer.context = specific.additionalContext;
  }
  if (event === "PermissionRequest" && (specific.decision?.behavior === "allow" || specific.decision?.behavior === "deny")) {
    answer.permission = { behavior: specific.decision.behavior, message: specific.decision.message };
  }
  return answer;
}

async function runHandler(event: string, handler: any, stdin: string, cwd: string): Promise<Outcome | null> {
  const run = await runCommand(handler, stdin, cwd);
  const blocking = BLOCKING.has(event);
  if (!run.ok || (run.status !== 0 && run.status !== 2)) {
    if (!blocking || handler.onFailure !== "block") return null;
    const detail = run.stderr.trim();
    return { block: `Blocked: hook ${HOOK.name} failed (${run.why})${detail ? `: ${detail}` : ""}` };
  }
  if (run.status === 2) {
    return blocking ? { block: run.stderr.trim() || `Blocked by hook ${HOOK.name}` } : null;
  }
  return readAnswer(event, run.stdout);
}

type Moment = {
  cwd: string;
  sessionId: string;
  // The value the group's matcher tests; undefined for an event with no matcher field.
  matcher?: string;
  // The Claude tool name and input, for `if`; undefined off a tool event.
  tool?: { name: string; input: any };
  fields: any;
};

// Runs the event's matching handlers in order. The first block (or deny) wins over any allow; the context
// of every handler before it is joined. A handler marked `async` runs without being awaited.
async function fire(event: string, moment: Moment): Promise<Outcome> {
  const stdin = JSON.stringify({
    session_id: moment.sessionId,
    cwd: moment.cwd,
    hook_event_name: event,
    ...moment.fields,
  });
  const contexts: string[] = [];
  const outcome: Outcome = {};
  for (const group of HOOK.events[event] ?? []) {
    if (moment.matcher !== undefined && !matcherAccepts(group.matcher, moment.matcher)) continue;
    for (const handler of group.handlers) {
      if (handler.if && moment.tool && !conditionHolds(handler.if, moment.tool.name, moment.tool.input)) continue;
      if (handler.async) {
        void runHandler(event, handler, stdin, moment.cwd);
        continue;
      }
      const answer = await runHandler(event, handler, stdin, moment.cwd);
      if (!answer) continue;
      if (answer.context) contexts.push(answer.context);
      if (answer.permission?.behavior === "allow") outcome.permission = answer.permission;
      if (answer.block || answer.permission?.behavior === "deny") {
        outcome.block = answer.block ?? answer.permission?.message ?? `Blocked by hook ${HOOK.name}`;
        outcome.permission = undefined;
        outcome.context = contexts.join("\n") || undefined;
        return outcome;
      }
    }
  }
  outcome.context = contexts.join("\n") || undefined;
  return outcome;
}
