# Bootstrap Docs

Bootstrap docs are setup instructions that skills or packages ship alongside their agents and skills. When a user runs `meridian bootstrap`, an agent session launches with all installed bootstrap docs injected into the system prompt. The bootstrap agent reads the instructions and walks the user through setup — checking installed tools, configuring harness settings, running first-time commands.

Bootstrap docs are read-only prompt context. They are never parsed as executable manifests.

## Two Tiers

Bootstrap docs come from two sources, loaded in this order:

1. **Skill-level** — `skills/<name>/resources/BOOTSTRAP.md` in a source package
2. **Package-level** — `bootstrap/<doc-name>/BOOTSTRAP.md` in a source package

### Skill-level bootstrap docs

Place a `BOOTSTRAP.md` inside a skill's `resources/` directory:

```
skills/my-skill/
  SKILL.md
  resources/
    BOOTSTRAP.md       # setup instructions specific to this skill
    other-resource.md  # other skill resources, unaffected
```

During `mars sync`, skill-level bootstrap docs copy to native harness directories alongside other skill resources:

```
.mars/skills/my-skill/resources/BOOTSTRAP.md       # canonical
.claude/skills/my-skill/resources/BOOTSTRAP.md     # native copy
.codex/skills/my-skill/resources/BOOTSTRAP.md      # native copy
```

This makes skill-level bootstrap docs visible to standalone mars users who read skill directories directly.

### Package-level bootstrap docs

Place a `BOOTSTRAP.md` inside a named subdirectory of `bootstrap/`:

```
bootstrap/
  image-generation/
    BOOTSTRAP.md       # e.g. instructions to enable image_generation in ~/.codex/config.toml
  workspace-setup/
    BOOTSTRAP.md       # e.g. instructions to configure workspace roots
```

During `mars sync`, package-level bootstrap docs sync to `.mars/bootstrap/`:

```
.mars/bootstrap/image-generation/BOOTSTRAP.md
.mars/bootstrap/workspace-setup/BOOTSTRAP.md
```

Package-level bootstrap docs are **not** copied to native harness directories. They are consumed by Meridian at launch time only.

### Manifest-declared bootstrap docs

Bootstrap docs can also be declared in package manifests instead of discovered from `bootstrap/<doc-name>/BOOTSTRAP.md`.

Supported declarations:

```toml
bootstrapDocs = ["./docs/global-auth"]
bootstrap_docs = ["./docs/workspace-setup"]

[package.bootstrap]
path = "./bootstrap"
```

Each path points to either:

- a bootstrap doc directory containing `BOOTSTRAP.md`
- a bootstrap container directory with child doc directories

## Writing a BOOTSTRAP.md

A bootstrap doc is plain markdown. Write it as instructions for an agent to follow when helping a user set up their environment.

Good bootstrap docs:
- Explain what needs to be configured and why
- Include the exact file paths, config keys, and values to check or set
- Note what the agent should verify before making changes (e.g. "check if X is already enabled")
- Give the agent enough context to explain the change to the user

```markdown
# Image Generation Setup

This skill requires `image_generation` to be enabled in your Codex configuration.

## Steps

1. Check `~/.codex/config.toml` for an `[capabilities]` section.
2. If `image_generation` is not set to `true`, prompt the user to enable it.
3. After the user confirms, add or update:
   ```toml
   [capabilities]
   image_generation = true
   ```
4. Confirm the change was saved.
```

## meridian bootstrap

`meridian bootstrap` launches a normal primary agent session with all installed bootstrap docs injected into the system prompt. No special UI or dedicated walkthrough engine — it's a regular agent session with the right context pre-loaded.

```bash
meridian bootstrap
meridian bootstrap -a my-bootstrap-agent   # use a specific agent profile
meridian bootstrap --model opus            # override model
```

The injected bootstrap docs appear in the system prompt in this order:
1. Skill-level docs, alphabetical by skill name
2. Package-level docs, alphabetical by doc name

Each doc is attributed with a `# Bootstrap: <name>` heading in the injected content.

The `--agent` flag selects which agent profile runs the session. If omitted, Meridian uses the default bootstrap agent from the installed agent catalog.

All standard primary launch flags (`--model`, `--harness`, `--approval`, `--work`, etc.) apply.

## Discovery and sync

Mars discovers bootstrap docs at the same time as agents and skills during `mars sync`. Any `bootstrap/` directory in a source package is automatically included; manifest declarations add docs outside that conventional location.

Skill-level bootstrap docs are part of the skill tree and require no extra configuration.

To verify what bootstrap docs are installed after sync:

```bash
ls .mars/bootstrap/                        # package-level docs
ls .mars/skills/*/resources/BOOTSTRAP.md  # skill-level docs
```
