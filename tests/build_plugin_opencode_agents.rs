mod common;

use assert_fs::TempDir;
use std::fs;

use common::*;

/// The OpenCode tree carries a published agent as plugin data, with OpenCode's v1 permission
/// keys translated to the v2 actions the plugin API matches on.
#[test]
fn opencode_plugin_registers_agent_with_v2_permissions() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("pkg");
    fs::create_dir_all(root.join("agents")).unwrap();
    fs::write(
        root.join("mars.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        root.join("agents/reader.md"),
        "---\nname: reader\ndescription: Reads\nmode: subagent\ndisallowed-tools: [bash, agent, write]\n---\nBody\n",
    )
    .unwrap();
    let out = dir.path().join("out");

    let output = offline_mars(dir.path())
        .args(["build", "plugin", "--root"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let index = fs::read_to_string(out.join("opencode/index.mjs")).unwrap();
    let data = index
        .split_once("const DATA = ")
        .and_then(|(_, rest)| rest.split_once("\nconst ROOT"))
        .map(|(data, _)| data.trim())
        .expect("index.mjs embeds DATA");
    let data: serde_json::Value = serde_json::from_str(data).unwrap();
    let agent = &data["agents"][0];
    assert_eq!(agent["id"], "pkg:reader");
    assert_eq!(agent["mode"], "subagent");
    let denied = agent["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| {
            (
                rule["action"].as_str().unwrap(),
                rule["effect"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        denied,
        [("shell", "deny"), ("subagent", "deny"), ("edit", "deny")]
    );
}
