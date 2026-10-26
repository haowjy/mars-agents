import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const DATA = /*__MARS_PLUGIN_DATA__*/
const ROOT = dirname(fileURLToPath(import.meta.url))
const HOOK_DIR_TOKEN = "${MARS_HOOK_DIR}"
const MCP_DIR_TOKEN = "${MARS_MCP_DIR}"

function substituteMcpDir(value, mcpDirectory) {
  if (typeof value === "string") return value.replaceAll(MCP_DIR_TOKEN, mcpDirectory)
  if (Array.isArray(value)) return value.map((item) => substituteMcpDir(item, mcpDirectory))
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, substituteMcpDir(item, mcpDirectory)]),
    )
  }
  return value
}

function runtimeServer(name, server) {
  const resolved = substituteMcpDir(server, join(ROOT, "mcp", name))
  if (!resolved.environment) return resolved

  const environment = {}
  for (const [key, value] of Object.entries(resolved.environment)) {
    const match = /^\{env:(.+)\}$/.exec(value)
    if (!match) {
      environment[key] = value
      continue
    }
    const resolvedValue = process.env[match[1]]
    if (resolvedValue !== undefined) environment[key] = resolvedValue
  }
  return { ...resolved, environment }
}

function encodeForJavaScriptString(value) {
  return value.replace(/[^A-Za-z0-9/._-]/g, (character) => {
    return `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`
  })
}

async function renderHook(hook) {
  const hookDirectory = join(ROOT, "hooks", hook.name)
  const source = await readFile(join(hookDirectory, hook.fragment), "utf8")
  return source.replaceAll(HOOK_DIR_TOKEN, encodeForJavaScriptString(hookDirectory))
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
          throw new Error(`OpenCode hook ${hook.name} must export default { id, setup }`)
        }
        const cleanup = await definition.setup(ctx)
        if (typeof cleanup === "function") cleanups.push(cleanup)
      } catch (error) {
        console.error(`[${DATA.plugin}] failed to load hook ${hook.name}`, error)
      }
    }

    if (cleanups.length) {
      return async () => {
        for (const cleanup of cleanups.reverse()) await cleanup()
      }
    }
  },
}
