import { createHash, randomUUID } from "node:crypto"
import { chmod, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

const DATA = /*__MARS_PLUGIN_DATA__*/
const ROOT = dirname(fileURLToPath(import.meta.url))
const HOOK_DIR_TOKEN = "${MARS_HOOK_DIR}"

function hash(content) {
  return createHash("sha256").update(content).digest("hex")
}

function runtimeServer(server) {
  if (!server.environment) return server

  const environment = {}
  for (const [key, value] of Object.entries(server.environment)) {
    const match = /^\$\{env:(.+)\}$/.exec(value)
    if (!match) {
      environment[key] = value
      continue
    }
    const resolved = process.env[match[1]]
    if (resolved !== undefined) environment[key] = resolved
  }
  return { ...server, environment }
}

async function verifiedWrite(directory, filename, content, expectedHash, privateDirectory) {
  if (privateDirectory) {
    await mkdir(directory, { recursive: true, mode: 0o700 })
    await chmod(directory, 0o700)
  }

  const destination = join(directory, filename)
  const existing = await readFile(destination, "utf8").catch(() => undefined)
  if (existing === undefined || hash(existing) !== expectedHash) {
    const temporary = join(directory, `.${filename}.${process.pid}.${randomUUID()}.tmp`)
    try {
      await writeFile(temporary, content, { flag: "wx", mode: 0o600 })
      await rename(temporary, destination)
    } finally {
      await rm(temporary, { force: true }).catch(() => {})
    }
  }

  const installed = await readFile(destination, "utf8")
  if (hash(installed) !== expectedHash) {
    throw new Error(`refusing to import modified rendered hook ${destination}`)
  }
  return destination
}

async function renderHook(hook) {
  const hookDirectory = join(ROOT, "hooks", hook.name)
  const source = await readFile(join(hookDirectory, hook.fragment), "utf8")
  const portableHookDirectory = hookDirectory.replaceAll("\\", "/")
  const rendered = source.replaceAll(HOOK_DIR_TOKEN, portableHookDirectory)
  const contentHash = hash(rendered)
  const filename = `${contentHash}.ts`
  const cacheRoot = process.env.XDG_CACHE_HOME || join(homedir(), ".cache")
  const cacheDirectory = join(cacheRoot, "opencode", "mars-hooks", hash(DATA.plugin))

  try {
    return await verifiedWrite(cacheDirectory, filename, rendered, contentHash, true)
  } catch {
    return verifiedWrite(hookDirectory, `.mars-rendered-${filename}`, rendered, contentHash, false)
  }
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
          editor.set(`${DATA.plugin}:${name}`, runtimeServer(servers[name]))
        }
      })
    }

    const cleanups = []
    for (const hook of DATA.hooks) {
      const rendered = await renderHook(hook)
      const definition = (await import(pathToFileURL(rendered).href)).default
      if (!definition || typeof definition.id !== "string" || typeof definition.setup !== "function") {
        throw new Error(`OpenCode hook ${hook.name} must export default { id, setup }`)
      }
      const cleanup = await definition.setup(ctx)
      if (typeof cleanup === "function") cleanups.push(cleanup)
    }

    if (cleanups.length) {
      return async () => {
        for (const cleanup of cleanups.reverse()) await cleanup()
      }
    }
  },
}
