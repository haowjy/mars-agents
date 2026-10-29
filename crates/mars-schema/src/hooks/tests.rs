use super::*;
use crate::diagnostic::Severity;

#[test]
fn opencode_native_hooks_need_a_default_export() {
    let opencode = targets::target(".opencode").unwrap();
    for ok in [
        "export default { id: 'a' }",
        "export { plugin as default }",
        "export * as default from './x'",
    ] {
        assert_eq!(opencode.native_hook_problem(ok), None, "{ok}");
    }
    assert!(opencode.native_hook_problem("export const x = 1").is_some());
    assert_eq!(
        targets::target(".pi")
            .unwrap()
            .native_hook_problem("anything"),
        None
    );
}

fn stop_hook(handlers: &str) -> String {
    format!("[[hooks.Stop]]\n{handlers}")
}

#[test]
fn handler_timeout_and_on_failure_fail_the_hook_for_both_origins() {
    let bad = |field: &str| {
        stop_hook(&format!(
            "[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"a\"\n{field}\n\
             [[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"b\"\n"
        ))
    };
    for field in [
        "timeout = 1e10",
        "timeout = 0",
        "timeout = -1",
        "timeout = 2147484",
        "onFailure = \"Block\"",
    ] {
        let local = read(&bad(field), "h", Origin::Local);
        assert!(local.def.is_none(), "{field}");
        assert_eq!(local.diagnostics[0].code, "hook.handler", "{field}");
        // A dependency's hook fails whole too, so the sync keeps its last good install.
        let dep = read(&bad(field), "h", Origin::Dependency);
        assert!(dep.def.is_none(), "{field}");
        assert_eq!(dep.diagnostics[0].code, "hook.handler", "{field}");
    }
    for ok in [
        "timeout = 2147483",
        "timeout = 0.5",
        "onFailure = \"continue\"",
    ] {
        assert!(read(&bad(ok), "h", Origin::Local).def.is_some(), "{ok}");
    }
}

#[test]
fn hook_names_reject_control_characters() {
    let text = stop_hook("[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"a\"\n");
    for name in ["a\\nb", "a\\u2028b", "a\\tb", "a\\u0007b", "a/b", ""] {
        let toml = format!("name = \"{name}\"\n{text}");
        let read = read(&toml, "dir", Origin::Local);
        assert!(read.def.is_none(), "{name:?}");
    }
    assert!(read(&text, "fine-name", Origin::Local).def.is_some());
}

fn tokens() -> Tokens<'static> {
    Tokens {
        hook_dir: "/p/.claude/hooks/audit",
        hook_target: ".claude",
        ..Tokens::default()
    }
}

/// A hook with one target table per `(root, unchecked)`, and `extra` top-level text.
fn def_with(extra: &str, tables: &[(&str, bool)]) -> HookDef {
    let mut text = format!("{extra}\n");
    for (root, unchecked) in tables {
        text.push_str(&format!("[targets.\"{root}\"]\nnative-hook = \"f.json\"\n"));
        if *unchecked {
            text.push_str("unchecked = true\n");
        }
    }
    read(&text, "audit", Origin::Local).def.expect(&text)
}

fn output(
    def: &HookDef,
    root: &str,
    native_hook: Option<&str>,
    origin: Origin,
) -> Result<MergeOutput, NativeHookError> {
    match def.output(
        targets::target(root).unwrap(),
        native_hook,
        origin,
        &tokens(),
    )? {
        HookOutput::Merge(output) => Ok(output),
        HookOutput::File(_) => panic!("{root} is a file target"),
    }
}

fn events(root: &str) -> &'static [targets::Event] {
    let target = targets::target(root).unwrap();
    assert!(target.merge_file().is_some(), "{root}");
    target.profile.events
}

fn profile(root: &str) -> targets::Profile {
    targets::target(root).unwrap().profile
}

#[test]
fn event_tables_match_supported_command_hook_targets() {
    let (claude, codex, cursor) = (events(".claude"), events(".codex"), events(".cursor"));
    assert_eq!(claude.len(), 29);
    assert!(claude.iter().any(|e| e.name == "SessionEnd"));
    assert_eq!(codex.len(), 11);
    assert!(codex.iter().any(|e| e.name == "SessionEnd"));
    assert_eq!(cursor.len(), 21);
    assert!(cursor.iter().any(|e| e.name == "beforeShellExecution"));
    for root in [".opencode", ".pi"] {
        assert!(targets::target(root).unwrap().is_file());
    }
    assert!(targets::target(".agents").is_none());
    // Every Codex event is a Claude event, so a Claude-keyed table renders on both.
    assert!(
        codex
            .iter()
            .all(|e| targets::claude_event(e.name).is_some())
    );
}

#[test]
fn file_targets_name_their_native_hook_and_generated_files() {
    let opencode = targets::target(".opencode").unwrap();
    assert_eq!(
        opencode.native_hook_path("audit").as_deref(),
        Some("plugins/mars-audit.ts")
    );
    assert_eq!(
        opencode.generated_path("audit").as_deref(),
        Some("plugins/mars-audit.hooks.ts")
    );
    let pi = targets::target(".pi").unwrap();
    assert_eq!(
        pi.native_hook_path("audit").as_deref(),
        Some("extensions/mars-audit.ts")
    );
    assert_eq!(
        targets::target(".codex").unwrap().native_hook_path("audit"),
        None
    );
}

#[test]
fn hook_dir_token_is_replaced_in_text_and_in_every_json_string() {
    let t = Tokens {
        hook_dir: "${CLAUDE_PLUGIN_ROOT}/hooks/audit",
        hook_target: "plugin",
        ..Tokens::default()
    };
    assert_eq!(
        t.substitute(r#"const SCRIPT = "${MARS_HOOK_DIR}/run.sh"; // ${MARS_HOOK_TARGET}"#),
        r#"const SCRIPT = "${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh"; // plugin"#
    );
    let def = def_with("", &[(".claude", false)]);
    let text = r#"{"Stop":[{"hooks":[{"command":"${MARS_HOOK_DIR}/run.sh"}]}]}"#;
    let merged = match def
        .output(
            targets::target(".claude").unwrap(),
            Some(text),
            Origin::Local,
            &t,
        )
        .unwrap()
    {
        HookOutput::Merge(merged) => merged,
        HookOutput::File(_) => unreachable!(),
    };
    assert_eq!(
        serde_json::to_string(&merged.events["Stop"]).unwrap(),
        r#"[{"hooks":[{"command":"${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh"}]}]"#
    );
}

#[test]
fn merge_emits_the_bytes_a_serde_json_value_would() {
    // The lock's `emitted_json` and Codex trust hash these bytes: keys sort, as they
    // did when native hooks were `serde_json::Value`s.
    let def = def_with("", &[(".claude", false)]);
    let text = r#"{"version":1,"hooks":{"PreToolUse":[
        {"matcher":"Bash","hooks":[{"timeout":5,"command":"${MARS_HOOK_DIR}/run.sh","type":"command"}]}]}}"#;
    let merged = output(&def, ".claude", Some(text), Origin::Local).unwrap();
    let entries: Vec<_> = merged.events["PreToolUse"]
        .iter()
        .map(OrderedValue::to_json)
        .collect();
    assert_eq!(
        serde_json::to_string(&entries).unwrap(),
        r#"[{"hooks":[{"command":"/p/.claude/hooks/audit/run.sh","timeout":5,"type":"command"}],"matcher":"Bash"}]"#
    );
}

#[test]
fn merge_refuses_unknown_events_unless_unchecked() {
    let text = r#"{"Bogus":[]}"#;
    let strict = def_with("", &[(".codex", false)]);
    let error = output(&strict, ".codex", Some(text), Origin::Local)
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("unknown event `Bogus` for target `.codex`"));
    let loose = def_with("", &[(".codex", true)]);
    let merged = output(&loose, ".codex", Some(text), Origin::Local).unwrap();
    assert_eq!(merged.unchecked_events, ["Bogus"]);
    // A dependency's unknown event is skipped; the project's is the error above.
    let skipped = output(&strict, ".codex", Some(text), Origin::Dependency).unwrap();
    assert_eq!(skipped.skipped_events, ["Bogus"]);
    assert!(skipped.events.is_empty());
}

#[test]
fn merge_refuses_non_objects_and_non_arrays() {
    let def = def_with("", &[(".claude", false)]);
    let run = |text| output(&def, ".claude", Some(text), Origin::Local);
    assert!(run("[]").is_err());
    assert!(run("nope").is_err());
    assert!(run(r#"{"Stop":{}}"#).is_err());
    // A wrapper with another key is an event table, so `hooks` is an unknown event.
    assert!(run(r#"{"hooks":{},"Stop":[]}"#).is_err());
}

fn read_local(text: &str) -> HookRead {
    read(text, "audit", Origin::Local)
}

fn read_dependency(text: &str) -> HookRead {
    read(text, "audit", Origin::Dependency)
}

fn message(got: &HookRead) -> &str {
    got.diagnostics[0].message()
}

const GUARD: &str = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "bash ${MARS_HOOK_DIR}/run.sh"

[[hooks.Notification]]
[[hooks.Notification.hooks]]
type = "command"
command = "echo n"
"#;

#[test]
fn reads_targets_with_default_native_hooks() {
    let got = read_local(
        "order = 3\n[targets.\".claude\"]\n[targets.\".pi\"]\nnative-hook = \"x/pi.ts\"\n",
    );
    let def = got.def.unwrap();
    assert_eq!((def.name.as_str(), def.order), ("audit", 3));
    assert_eq!(def.native_hook(".claude").unwrap().file, "claude.json");
    assert_eq!(def.native_hook(".pi").unwrap().file, "x/pi.ts");
    assert!(def.native_hook(".codex").is_none());
    let managed = [".pi".to_string(), ".codex".to_string()];
    assert_eq!(def.select(&managed), [".pi"]);
}

#[test]
fn hooks_select_every_managed_target_that_renders_an_event() {
    let def = read_local(GUARD).def.unwrap();
    let managed: Vec<String> = [".claude", ".codex", ".cursor", ".opencode", ".pi"]
        .map(String::from)
        .into();
    // Cursor renders nothing before 0.26; GUARD's PreToolUse renders on the two TypeScript targets.
    assert_eq!(
        def.select(&managed),
        [".claude", ".codex", ".opencode", ".pi"]
    );
    let skipped = read_local(&format!("skip-targets = [\".codex\"]\n{GUARD}"))
        .def
        .unwrap();
    assert_eq!(skipped.select(&managed), [".claude", ".opencode", ".pi"]);
    assert!(skipped.skips_for(".codex").is_empty());
    assert_eq!(def.skips_for(".codex").len(), 1);
}

#[test]
fn a_native_hook_still_installs_on_a_skipped_target() {
    let def = read_local(&format!(
        "skip-targets = [\".codex\"]\n{GUARD}\n[targets.\".codex\"]\nnative-hook = \"codex.json\"\n"
    ))
    .def
    .unwrap();
    let managed = [".codex".to_string()];
    assert_eq!(def.select(&managed), [".codex"]);
    let out = output(&def, ".codex", Some(r#"{"Stop":[]}"#), Origin::Local).unwrap();
    // Skipped: no generated PreToolUse. The native hook's own event is all there is.
    assert_eq!(out.events.keys().collect::<Vec<_>>(), ["Stop"]);
}

#[test]
fn a_native_hook_replaces_only_the_events_it_declares() {
    let def = read_local(&format!(
        "{GUARD}\n[targets.\".codex\"]\nnative-hook = \"codex.json\"\n"
    ))
    .def
    .unwrap();
    let native_hook = r#"{"PreToolUse":[],"Stop":[{"hooks":[]}]}"#;
    let out = output(&def, ".codex", Some(native_hook), Origin::Local).unwrap();
    // The empty array is kept, so the caller sees the event turned off.
    assert_eq!(out.events["PreToolUse"], Vec::<OrderedValue>::new());
    assert_eq!(out.overrides, ["PreToolUse"]);
    assert!(out.events.contains_key("Stop"));
    // The override check also needs a declared event: `Stop` is not in `[hooks]`.
    let untouched = output(&def, ".claude", None, Origin::Local).unwrap();
    assert_eq!(untouched.events.len(), 2);
    assert!(untouched.overrides.is_empty());
}

#[test]
fn hooks_need_explicit_native_hook_names() {
    let got = read_local(&format!("{GUARD}\n[targets.\".codex\"]\n"));
    assert!(got.def.is_none());
    assert!(message(&got).contains("must name its `native-hook`"));
    // Without `[hooks]` an empty table keeps today's meaning.
    assert!(read_local("[targets.\".codex\"]\n").def.is_some());
}

#[test]
fn a_hook_needs_hooks_or_a_target_table() {
    for text in ["", "[hooks]\n", "[targets]\n"] {
        let got = read_local(text);
        assert!(got.def.is_none(), "{text}");
        assert!(message(&got).contains("at least one target"), "{text}");
    }
}

#[test]
fn unknown_events_are_strict_for_the_project_and_skipped_for_a_dependency() {
    let got = read_local(GUARD);
    assert!(got.def.is_some(), "Notification is a Claude event");
    let text = format!(
        "{GUARD}\n[[hooks.FutureEvent]]\n[[hooks.FutureEvent.hooks]]\ntype = \"command\"\ncommand = \"x\"\n"
    );
    let got = read_local(&text);
    assert!(got.def.is_none());
    assert!(message(&got).contains("unknown event `FutureEvent`"));

    let got = read_dependency(&text);
    let def = got.def.unwrap();
    assert_eq!(got.diagnostics.len(), 1);
    assert_eq!(got.diagnostics[0].code, "hook.unknown-event");
    assert!(!def.portable().unwrap().declares("FutureEvent"));
    assert!(def.portable().unwrap().declares("PreToolUse"));

    // `unchecked = true` keeps it: Claude renders it, Codex reports it.
    let def = read_local(&format!("unchecked = true\n{text}"))
        .def
        .unwrap();
    let portable = def.portable().unwrap();
    let claude = portable.render(&profile(".claude"), &tokens());
    let render::Output::Merge {
        unchecked_events, ..
    } = &claude.output
    else {
        panic!("Claude is a merge target");
    };
    assert_eq!(unchecked_events, &["FutureEvent"]);
    let codex = portable.render(&profile(".codex"), &tokens());
    assert!(!codex.events().contains_key("FutureEvent"));
    assert!(
        codex
            .skips
            .iter()
            .any(|s| s.event.as_deref() == Some("FutureEvent"))
    );
}

#[test]
fn skip_targets_must_be_target_roots() {
    let got = read_local(&format!("skip-targets = [\"codex\"]\n{GUARD}"));
    assert!(got.def.is_none());
    assert!(message(&got).contains("not a managed target root"));
}

#[test]
fn unchecked_on_a_file_target_is_an_error_for_the_project_and_a_warning_for_a_dependency() {
    let text = "[targets.\".pi\"]\nnative-hook = \"p.ts\"\nunchecked = false\n";
    let got = read_local(text);
    assert!(got.def.is_none());
    assert!(message(&got).contains("`unchecked` is not supported"));
    let got = read_dependency(text);
    assert!(got.def.is_some());
    assert_eq!(got.diagnostics[0].severity, Severity::Warning);
}

#[test]
fn a_dependency_hook_fails_validation_on_a_bad_handler() {
    let got = read_dependency("[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ncommand = \"x\"\n");
    assert!(got.def.is_none());
    assert!(message(&got).contains("`type` is required"));
}

#[test]
fn reserves_the_hooks_suffix_in_names() {
    let got = read_local("name = \"audit.hooks\"\n[targets.\".claude\"]\n");
    assert!(got.def.is_none());
    assert!(message(&got).contains("`audit.hooks`"));
    assert!(
        read("[targets.\".claude\"]\n", "dir.hooks", Origin::Local)
            .def
            .is_none()
    );
}

#[test]
fn diagnostics_carry_no_path() {
    for (text, needle) in [
        ("targets = [", "failed to parse"),
        (
            "[targets.\".claude\"]\nbogus = 1\n",
            "unknown field `bogus`",
        ),
        ("[targets]\n", "at least one target"),
        (
            "[targets.\".claude\"]\nnative-hook = \"../x\"\n",
            "invalid native hook",
        ),
    ] {
        let got = read_local(text);
        assert!(got.def.is_none(), "{text}");
        assert!(message(&got).contains(needle), "{}", message(&got));
    }
    let removed = read_local("[[targets]]\nevent = \"x\"\n");
    assert_eq!(removed.diagnostics[0].code, REMOVED_SCHEMA_CODE);
}

#[test]
fn unknown_visibility_reads_as_local() {
    let got = read_local("visibility = \"public\"\n[targets.\".claude\"]\n");
    assert_eq!(got.def.unwrap().visibility, Visibility::Local);
    assert_eq!(got.unknown_visibility.as_deref(), Some("public"));
}

#[test]
fn an_empty_file_target_table_needs_a_native_hook_for_both_origins() {
    let text = format!(
        "{}[targets.\".opencode\"]\n",
        stop_hook("[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"a\"\n")
    );
    for origin in [Origin::Local, Origin::Dependency] {
        let read = read(&text, "h", origin);
        assert!(read.def.is_none());
        assert_eq!(read.diagnostics[0].code, "hook.native-hook-required");
    }
}

#[test]
fn fragment_was_removed_for_both_origins() {
    for (text, fix) in [
        (
            "[targets.\".codex\"]\nfragment = \"old.json\"\n",
            "use `native-hook`",
        ),
        (
            "[targets.\".codex\"]\nnative-hook = \"a.json\"\nfragment = \"b.json\"\n",
            "delete it, `native-hook` is already set",
        ),
    ] {
        for got in [read_local(text), read_dependency(text)] {
            assert!(got.def.is_none());
            assert_eq!(got.diagnostics.len(), 1);
            assert_eq!(got.diagnostics[0].code, "field.removed");
            assert_eq!(got.diagnostics[0].severity, Severity::Error);
            assert_eq!(
                message(&got),
                format!("target `.codex`: `fragment` was removed in 0.27; {fix}")
            );
        }
    }
}

#[test]
fn native_hook_names_the_file_in_a_merge_target() {
    let got = read_local("[targets.\".codex\"]\nnative-hook = \"a.json\"\n");
    assert_eq!(
        got.def.unwrap().native_hook(".codex").unwrap().file,
        "a.json"
    );
}
