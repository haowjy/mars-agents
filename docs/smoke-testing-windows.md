# Smoke Testing (Windows PowerShell)

This page documents manual smoke checks for Windows. These cover the same scenarios as the [POSIX guide](smoke-testing-posix.md) but use PowerShell syntax.

Use these when:
- Testing Windows-specific cache path handling
- Verifying readonly file replacement
- Checking explicit-port URL cache naming
- Preparing a Windows release

## Baseline

Always start with the deterministic checks:

```powershell
cargo fmt --all --check
cargo test -q
```

## Local Path Add

Verifies local source parsing, discovery, and install on Windows.

```powershell
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "mars-smoke-$([guid]::NewGuid())")
$proj = Join-Path $tmp "project"
$src = Join-Path $tmp "source"

New-Item -ItemType Directory -Path "$proj" | Out-Null
New-Item -ItemType Directory -Path "$src\skills\planning" -Force | Out-Null
New-Item -ItemType Directory -Path "$src\agents" -Force | Out-Null

@"
---
name: planning
description: local planning
---
# Planning
"@ | Set-Content -Path "$src\skills\planning\SKILL.md"

@"
---
name: coder
description: local coder
skills:
  - planning
---
# Coder
"@ | Set-Content -Path "$src\agents\coder.md"

mars init --root $proj
mars add $src --root $proj
mars doctor --root $proj

# Cleanup
Remove-Item -Recurse -Force $tmp
```

Expected result:
- `mars add` succeeds
- `mars doctor` exits cleanly

## Backslash Path Syntax

Verifies Windows backslash relative paths are recognized as local sources.

```powershell
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "mars-smoke-$([guid]::NewGuid())")
$proj = Join-Path $tmp "project"

New-Item -ItemType Directory -Path "$proj" | Out-Null
New-Item -ItemType Directory -Path "$tmp\mysrc\skills\test" -Force | Out-Null

@"
---
name: test
description: test skill
---
# Test
"@ | Set-Content -Path "$tmp\mysrc\skills\test\SKILL.md"

mars init --root $proj

# Use backslash relative path
Set-Location $tmp
mars add ".\mysrc" --root $proj
mars doctor --root $proj

# Cleanup
Remove-Item -Recurse -Force $tmp
```

Expected result:
- Backslash path recognized as local source
- `mars add` and `mars doctor` succeed

## Explicit-Port Cache Naming

Verifies that explicit-port URLs produce cache directory names without colons.

```powershell
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "mars-smoke-$([guid]::NewGuid())")
$proj = Join-Path $tmp "project"

# Note: This test requires a git daemon on port 19424.
# If you don't have one, check the cache directory naming manually after
# adding any explicit-port source.

mars init --root $proj

# After adding a source with explicit port, verify cache directory names:
Get-ChildItem (Join-Path $env:LOCALAPPDATA "mars\cache\git") -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match ':' } |
  ForEach-Object { Write-Error "Colon in cache dir: $($_.Name)" }

# Cleanup
Remove-Item -Recurse -Force $tmp
```

Expected result:
- Cache directory names contain no colons
- All cache subdirectories are valid Windows path components

## Readonly File Replacement

Verifies that sync can replace files marked as readonly.

```powershell
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "mars-smoke-$([guid]::NewGuid())")
$proj = Join-Path $tmp "project"
$src = Join-Path $tmp "source"

New-Item -ItemType Directory -Path "$proj\.agents\skills\test" -Force | Out-Null
New-Item -ItemType Directory -Path "$src\skills\test" -Force | Out-Null

@"
---
name: test
description: original
---
# Original
"@ | Set-Content -Path "$proj\.agents\skills\test\SKILL.md"

# Mark as readonly
Set-ItemProperty -Path "$proj\.agents\skills\test\SKILL.md" -Name IsReadOnly -Value $true

@"
---
name: test
description: updated
---
# Updated
"@ | Set-Content -Path "$src\skills\test\SKILL.md"

mars init --root $proj
mars add $src --root $proj
mars sync --root $proj

# Verify the readonly file was replaced
$content = Get-Content "$proj\.agents\skills\test\SKILL.md" -Raw
if ($content -notmatch 'updated') { Write-Error "Readonly replacement failed" }
else { Write-Host "OK: readonly file replaced successfully" }

# Cleanup
Remove-Item -Recurse -Force $tmp
```

Expected result:
- Sync replaces readonly file successfully
- Output shows `OK: readonly file replaced successfully`

## GitHub Repo Add

Verifies hosted source fetch on Windows.

```powershell
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "mars-smoke-$([guid]::NewGuid())")

mars init --root $tmp
mars add meridian-flow/meridian-base --root $tmp
mars doctor --root $tmp

# Cleanup
Remove-Item -Recurse -Force $tmp
```

Expected result:
- `mars add` succeeds
- `mars doctor` exits cleanly

## When To Run Which Checks

Run **Local Path Add** and **Backslash Path Syntax** for path classification changes.

Run **Explicit-Port Cache Naming** for cache component generation changes.

Run **Readonly File Replacement** for fs_ops or directory replacement changes.

Run **GitHub Repo Add** as a general Windows sanity check before release.
