import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const DATA = /*__MARS_PLUGIN_DATA__*/
const ROOT = dirname(fileURLToPath(import.meta.url))
const HOOK_DIR_TOKEN = DATA.tokens.hookDir
const HOOK_TARGET_TOKEN = DATA.tokens.hookTarget
const MCP_DIR_TOKEN = DATA.tokens.mcpDir

function substituteMcpDir(value, mcpDirectory) {
  if (typeof value === "string") return value.replaceAll(MCP_DIR_TOKEN, () => mcpDirectory)
  if (Array.isArray(value)) return value.map((item) => substituteMcpDir(item, mcpDirectory))
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, substituteMcpDir(item, mcpDirectory)]),
    )
  }
  return value
}

const ENV_REF = /\{env:([^}]+)\}/g

// Resolve every `{env:VAR}` in a value, or return undefined when one is unset.
function resolveEnvRefs(value) {
  let missing = false
  const resolved = value.replace(ENV_REF, (_, name) => {
    const found = process.env[name]
    if (found === undefined) missing = true
    return found ?? ""
  })
  return missing ? undefined : resolved
}

function resolveEntries(values) {
  const resolved = {}
  for (const [key, value] of Object.entries(values)) {
    const found = resolveEnvRefs(value)
    if (found !== undefined) resolved[key] = found
  }
  return resolved
}

function runtimeServer(name, server) {
  const resolved = substituteMcpDir(server, join(ROOT, "mcp", name))
  if (resolved.type === "remote") {
    return resolved.headers ? { ...resolved, headers: resolveEntries(resolved.headers) } : resolved
  }
  return resolved.environment
    ? { ...resolved, environment: resolveEntries(resolved.environment) }
    : resolved
}

function encodeForJavaScriptString(value) {
  return value.replace(/[^A-Za-z0-9/._-]/g, (character) => {
    return `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`
  })
}

async function renderHook(hook) {
  const hookDirectory = join(ROOT, "hooks", hook.name)
  const source = await readFile(join(hookDirectory, hook.file), "utf8")
  return source
    .replaceAll(HOOK_TARGET_TOKEN, () => ".opencode")
    .replaceAll(HOOK_DIR_TOKEN, encodeForJavaScriptString(hookDirectory))
}

export default {
  id: DATA.plugin,
  async setup(ctx) {
    if (DATA.skills.length) {
      const skills = await Promise.all(
        DATA.skills.map(async (skill) => {
          const path = join(ROOT, "skills", skill.directory, "SKILL.md")
          return { ...skill, path, content: await readFile(path, "utf8") }
        }),
      )
      await ctx.skill.transform((editor) => {
        for (const skill of skills) {
          editor.add({
            id: `${DATA.plugin}:${skill.directory}`,
            name: skill.name,
            description: skill.description,
            path: skill.path,
            content: skill.content,
          })
        }
      })
    }

    if (DATA.agents.length) {
      await ctx.agent.transform((editor) => {
        for (const agent of DATA.agents) {
          editor.update(agent.id, (info) => {
            info.name = agent.name
            info.description = agent.description
            info.mode = agent.mode
            if (agent.model) info.model = agent.model
            info.system = agent.system
            info.permissions.push(...agent.permissions)
          })
        }
      })
    }

    if (DATA.mcp.length) {
      const servers = JSON.parse(await readFile(join(ROOT, "mcp.json"), "utf8"))
      await ctx.mcp.transform((editor) => {
        for (const name of DATA.mcp) {
          editor.set(`${DATA.plugin}:${name}`, runtimeServer(name, servers[name]))
        }
      })
    }

    const cleanups = []
    for (const hook of DATA.hooks) {
      try {
        const rendered = await renderHook(hook)
        const url = `data:text/typescript;base64,${Buffer.from(rendered, "utf8").toString("base64")}`
        const definition = (await import(url)).default
        if (!definition || typeof definition.id !== "string" || typeof definition.setup !== "function") {
          throw new Error(`OpenCode hook ${hook.name}/${hook.file} must export default { id, setup }`)
        }
        const cleanup = await definition.setup(ctx)
        if (typeof cleanup === "function") cleanups.push(cleanup)
      } catch (error) {
        console.error(`[${DATA.plugin}] failed to load hook ${hook.name}/${hook.file}`, error)
      }
    }

    if (cleanups.length) {
      return async () => {
        for (const cleanup of cleanups.reverse()) await cleanup()
      }
    }
  },
}
