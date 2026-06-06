//! On-disk native agent manifest (`.mars/native-agents.json`) — schema, lock projection, queries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::lock::CompiledNativeOutput;

const NATIVE_AGENT_MANIFEST_VERSION: u32 = 1;
const NATIVE_AGENT_MANIFEST_FILENAME: &str = "native-agents.json";

#[derive(Debug, Serialize, Deserialize)]
struct NativeAgentManifestFile {
    version: u32,
    agents: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeAgentManifestRead {
    Found(BTreeMap<String, Vec<String>>),
    Missing,
    Unreadable,
}

/// Agent identity for manifest keys — must match inventory partition keys.
///
/// Native compile writes to `agents/{logical_name}.{ext}` where logical name is
/// `profile.name` (falling back to the canonical file stem). Inventory lookup
/// uses the same identity, so the manifest must derive from the native dest
/// path, not the canonical `.mars` owner path (which follows on-disk filename).
fn agent_name_from_native_dest(path: &str) -> Option<String> {
    let name = path.strip_prefix("agents/")?;
    let stem = Path::new(name).file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_string())
}

/// Native harness outputs recorded in the lock for agent items (canonical + linked targets).
fn compiled_native_outputs_from_lock(lock: &crate::lock::LockFile) -> Vec<CompiledNativeOutput> {
    use crate::lock::{CANONICAL_TARGET_ROOT, ItemKind};

    let mut records = Vec::new();
    for item in lock.items.values() {
        if item.kind != ItemKind::Agent {
            continue;
        }
        let Some(owner_canonical_dest_path) = item
            .outputs
            .iter()
            .find(|output| {
                output.target_root == CANONICAL_TARGET_ROOT
                    && output.dest_path.as_str().starts_with("agents/")
            })
            .map(|output| output.dest_path.to_string())
        else {
            continue;
        };
        for output in &item.outputs {
            if output.target_root == CANONICAL_TARGET_ROOT {
                continue;
            }
            if HarnessKind::from_target_dir(&output.target_root).is_none() {
                continue;
            }
            records.push(CompiledNativeOutput {
                owner_canonical_dest_path: owner_canonical_dest_path.clone(),
                target_root: output.target_root.clone(),
                dest_path: output.dest_path.to_string(),
                installed_checksum: output.installed_checksum.clone(),
            });
        }
    }
    records
}

fn manifest_agents_from_records(records: &[CompiledNativeOutput]) -> BTreeMap<String, Vec<String>> {
    let mut agents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in records {
        let Some(agent_name) = agent_name_from_native_dest(&record.dest_path) else {
            continue;
        };
        let Some(harness_kind) = HarnessKind::from_target_dir(&record.target_root) else {
            continue;
        };
        let harness = harness_kind.to_harness_id().as_str().to_string();
        let harnesses = agents.entry(agent_name).or_default();
        if !harnesses.iter().any(|existing| existing == &harness) {
            harnesses.push(harness);
        }
    }
    for harnesses in agents.values_mut() {
        harnesses.sort();
    }
    agents
}

fn manifest_path(mars_dir: &Path) -> PathBuf {
    mars_dir.join(NATIVE_AGENT_MANIFEST_FILENAME)
}

fn read_native_agent_manifest_state(mars_dir: &Path) -> NativeAgentManifestRead {
    let path = manifest_path(mars_dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return NativeAgentManifestRead::Missing,
    };
    let parsed: NativeAgentManifestFile = match serde_json::from_str(&content) {
        Ok(parsed) => parsed,
        Err(_) => return NativeAgentManifestRead::Unreadable,
    };
    if parsed.version != NATIVE_AGENT_MANIFEST_VERSION {
        return NativeAgentManifestRead::Unreadable;
    }
    NativeAgentManifestRead::Found(parsed.agents)
}

fn write_native_agent_manifest_file(
    project_root: &Path,
    manifest: &NativeAgentManifestFile,
) -> Result<(), MarsError> {
    let mars_dir = project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let path = manifest_path(&mars_dir);
    let json = serde_json::to_string_pretty(manifest).map_err(|err| {
        MarsError::Config(ConfigError::Invalid {
            message: format!("failed to serialize native agent manifest: {err}"),
        })
    })?;
    crate::fs::atomic_write(&path, json.as_bytes())
}

/// Write `.mars/native-agents.json` from the authoritative lock native-output set.
pub fn write_native_agent_manifest_from_lock(
    project_root: &Path,
    lock: &crate::lock::LockFile,
) -> Result<(), MarsError> {
    let manifest = NativeAgentManifestFile {
        version: NATIVE_AGENT_MANIFEST_VERSION,
        agents: manifest_agents_from_records(&compiled_native_outputs_from_lock(lock)),
    };
    write_native_agent_manifest_file(project_root, &manifest)
}

/// Persist the lock first, then best-effort write the native-agent manifest projection.
pub fn persist_lock_then_native_agent_manifest(
    project_root: &Path,
    lock: &crate::lock::LockFile,
) -> Result<Option<String>, MarsError> {
    crate::lock::write(project_root, lock)?;
    match write_native_agent_manifest_from_lock(project_root, lock) {
        Ok(()) => Ok(None),
        Err(err) => Ok(Some(format!(
            "could not write native agent manifest: {err}"
        ))),
    }
}

/// Read `.mars/native-agents.json` for inventory rendering and diagnostics.
pub fn read_native_agent_manifest(mars_dir: &Path) -> BTreeMap<String, Vec<String>> {
    match read_native_agent_manifest_state(mars_dir) {
        NativeAgentManifestRead::Found(agents) => agents,
        NativeAgentManifestRead::Missing | NativeAgentManifestRead::Unreadable => BTreeMap::new(),
    }
}

/// Whether `agent_name` is materialized natively for `harness` (HarnessId snake_case string).
pub fn agent_is_native_for_harness(
    manifest: &BTreeMap<String, Vec<String>>,
    agent_name: &str,
    harness: &str,
) -> bool {
    manifest
        .get(agent_name)
        .is_some_and(|harnesses| harnesses.iter().any(|entry| entry == harness))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::HarnessKind;
    use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
    use tempfile::TempDir;

    fn lock_with_agent_outputs(
        agent_key: &str,
        canonical_dest: &str,
        native_targets: &[(&str, &str)],
    ) -> LockFile {
        let mut lock = LockFile::empty();
        let mut outputs = vec![OutputRecord {
            target_root: ".mars".to_string(),
            dest_path: canonical_dest.into(),
            installed_checksum: "sha256:src".into(),
        }];
        for (target_root, dest_path) in native_targets {
            outputs.push(OutputRecord {
                target_root: (*target_root).to_string(),
                dest_path: (*dest_path).into(),
                installed_checksum: "sha256:native".into(),
            });
        }
        lock.items.insert(
            agent_key.to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs,
            },
        );
        lock
    }

    #[test]
    fn manifest_round_trips_through_read() {
        let dir = TempDir::new().unwrap();
        let lock = lock_with_agent_outputs(
            "agent/coder",
            "agents/coder.md",
            &[
                (HarnessKind::Claude.target_dir(), "agents/coder.md"),
                (HarnessKind::Codex.target_dir(), "agents/coder.toml"),
            ],
        );
        let lock = {
            let mut lock = lock;
            lock.items.insert(
                "agent/frontend-coder".to_string(),
                LockedItemV2 {
                    source: "test".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:src".into(),
                    outputs: vec![
                        OutputRecord {
                            target_root: ".mars".to_string(),
                            dest_path: "agents/frontend-coder.md".into(),
                            installed_checksum: "sha256:src".into(),
                        },
                        OutputRecord {
                            target_root: HarnessKind::Claude.target_dir().to_string(),
                            dest_path: "agents/frontend-coder.md".into(),
                            installed_checksum: "sha256:native".into(),
                        },
                    ],
                },
            );
            lock
        };

        write_native_agent_manifest_from_lock(dir.path(), &lock).unwrap();

        let manifest = read_native_agent_manifest(&dir.path().join(".mars"));
        let coder_harnesses: Option<Vec<&str>> = manifest
            .get("coder")
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(
            coder_harnesses.as_deref(),
            Some(["claude", "codex"].as_slice())
        );
        let frontend_harnesses: Option<Vec<&str>> = manifest
            .get("frontend-coder")
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(frontend_harnesses.as_deref(), Some(["claude"].as_slice()));
    }

    #[test]
    fn manifest_from_lock_reflects_authoritative_lock_not_stale_file() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".mars")).unwrap();
        std::fs::write(
            dir.path().join(".mars").join("native-agents.json"),
            r#"{"version":1,"agents":{"coder":["codex"]}}"#,
        )
        .unwrap();

        let lock = lock_with_agent_outputs(
            "agent/coder",
            "agents/coder.md",
            &[(HarnessKind::Claude.target_dir(), "agents/coder.md")],
        );

        write_native_agent_manifest_from_lock(dir.path(), &lock).unwrap();

        let manifest = read_native_agent_manifest(&dir.path().join(".mars"));
        let coder_harnesses: Option<Vec<&str>> = manifest
            .get("coder")
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(coder_harnesses.as_deref(), Some(["claude"].as_slice()));
        assert!(
            !manifest
                .get("coder")
                .is_some_and(|h| h.iter().any(|harness| harness == "codex")),
            "stale codex entry must be replaced by lock projection"
        );
    }

    #[test]
    fn manifest_uses_logical_name_from_native_dest_not_canonical_filename() {
        let dir = TempDir::new().unwrap();
        let lock = lock_with_agent_outputs(
            "agent/my-file",
            "agents/my-file.md",
            &[(HarnessKind::Claude.target_dir(), "agents/logical-name.md")],
        );

        write_native_agent_manifest_from_lock(dir.path(), &lock).unwrap();

        let manifest = read_native_agent_manifest(&dir.path().join(".mars"));
        let logical_harnesses: Option<Vec<&str>> = manifest
            .get("logical-name")
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(logical_harnesses.as_deref(), Some(["claude"].as_slice()));
        assert!(
            !manifest.contains_key("my-file"),
            "manifest must not key by canonical filename when native dest uses profile name"
        );
    }

    #[test]
    fn agent_is_native_for_harness_matches_manifest_membership() {
        let mut manifest = BTreeMap::new();
        manifest.insert("coder".to_string(), vec!["claude".to_string()]);
        assert!(agent_is_native_for_harness(&manifest, "coder", "claude"));
        assert!(!agent_is_native_for_harness(&manifest, "coder", "codex"));
        assert!(!agent_is_native_for_harness(
            &manifest, "explorer", "claude"
        ));
    }

    #[test]
    fn manifest_json_keys_are_sorted() {
        let dir = TempDir::new().unwrap();
        let lock = lock_with_agent_outputs(
            "agent/z-agent",
            "agents/z-agent.md",
            &[(HarnessKind::Claude.target_dir(), "agents/z-agent.md")],
        );
        let lock = {
            let mut lock = lock;
            lock.items.insert(
                "agent/a-agent".to_string(),
                LockedItemV2 {
                    source: "test".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:src".into(),
                    outputs: vec![
                        OutputRecord {
                            target_root: ".mars".to_string(),
                            dest_path: "agents/a-agent.md".into(),
                            installed_checksum: "sha256:src".into(),
                        },
                        OutputRecord {
                            target_root: HarnessKind::Claude.target_dir().to_string(),
                            dest_path: "agents/a-agent.md".into(),
                            installed_checksum: "sha256:native".into(),
                        },
                    ],
                },
            );
            lock
        };

        write_native_agent_manifest_from_lock(dir.path(), &lock).unwrap();

        let json = std::fs::read_to_string(dir.path().join(".mars/native-agents.json")).unwrap();
        assert!(
            json.find("\"a-agent\"").unwrap() < json.find("\"z-agent\"").unwrap(),
            "manifest JSON must emit agent keys in sorted order"
        );
    }
}
