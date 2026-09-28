use super::*;
use crate::models::possible::Provenance;
use tempfile::TempDir;

fn row(harness: HarnessId, launch: &str, provider: Option<&str>, bare: &str) -> PossibleRow {
    PossibleRow {
        harness,
        harness_model_id: launch.into(),
        provider: provider.map(str::to_string),
        model_id: bare.into(),
        provenance: Provenance::Inferred {
            catalog_fetched_at: Some("123".into()),
        },
    }
}

fn rules(user: &str, project: &str, local: &str) -> (TempDir, CuratedRules) {
    let temp = TempDir::new().unwrap();
    let paths = [
        temp.path().join("user.toml"),
        temp.path().join("mars.curated.toml"),
        temp.path().join("mars.curated.local.toml"),
    ];
    for (path, content) in paths.iter().zip([user, project, local]) {
        if !content.is_empty() {
            std::fs::write(path, content).unwrap();
        }
    }
    let rules = CuratedRules::load_paths(&paths[0], &paths[1], &paths[2]).unwrap();
    (temp, rules)
}

fn decision(rules: &CuratedRules, row: &PossibleRow, all: &[PossibleRow]) -> Decision {
    rules.matcher(all).decide(&RowKey::from(row))
}

#[test]
fn every_within_tier_specificity_boundary_is_order_independent() {
    let rows = vec![
        row(HarnessId::Codex, "gpt-5-mini", Some("openai"), "gpt-5-mini"),
        row(HarnessId::Codex, "gpt-5", Some("openai"), "gpt-5"),
        row(HarnessId::Codex, "gpt-6", Some("openai"), "gpt-6"),
    ];
    for (text, expected) in [
        (
            "[[show]]\nharness='codex'\nmodel='gpt-*'\n[[hide]]\nharness='codex'\nmodel='gpt-5*'\n",
            [true, true, false],
        ),
        (
            "[[hide]]\nharness='codex'\nmodel='gpt-5*'\n[[show]]\nharness='codex'\nmodel='gpt-*'\n",
            [true, true, false],
        ),
        (
            "[[hide]]\nharness='codex'\nmodel='gpt-5*'\n[[show]]\nharness='codex'\nmodel='gpt-5'\n",
            [true, false, false],
        ),
        (
            "[[show]]\nharness='codex'\nmodel='gpt-5'\n[[hide]]\nharness='codex'\nmodel='gpt-5'\n",
            [false, true, false],
        ),
    ] {
        let (_, rules) = rules("", text, "");
        for (row, hidden) in rows.iter().zip(expected) {
            assert_eq!(
                matches!(decision(&rules, row, &rows), Decision::Hidden { .. }),
                hidden,
                "{text}: {}",
                row.model_id
            );
        }
    }
}

#[test]
fn worked_example_folds_user_project_local_without_glob_regression() {
    let user =
        "[[hide]]\nharness='*'\nmodel='*-preview*'\n[[show]]\nharness='pi'\nmodel='deepseek/*'\n";
    let project = "[[show]]\nharness='codex'\nmodel='gpt-6-*'\n[[hide]]\nharness='codex'\nmodel='gpt-6-luna'\n";
    let local = "[[show]]\nharness='codex'\nmodel='gpt-6-luna'\n[[show]]\nharness='pi'\nmodel='openai-codex/gpt-5.6-sol'\n";
    let (_, curated) = rules(user, project, local);
    let rows = vec![
        row(HarnessId::Codex, "gpt-6-sol", Some("openai"), "gpt-6-sol"),
        row(HarnessId::Codex, "gpt-6-luna", Some("openai"), "gpt-6-luna"),
        row(
            HarnessId::Codex,
            "gpt-6-sol-preview",
            Some("openai"),
            "gpt-6-sol-preview",
        ),
        row(
            HarnessId::Pi,
            "deepseek/deepseek-flash",
            Some("deepseek"),
            "deepseek-flash",
        ),
        row(
            HarnessId::Pi,
            "openai-codex/gpt-5.6-sol",
            Some("openai-codex"),
            "gpt-5.6-sol",
        ),
        row(
            HarnessId::Pi,
            "openai-codex/gpt-5.5",
            Some("openai-codex"),
            "gpt-5.5",
        ),
    ];
    assert!(matches!(
        decision(&curated, &rows[0], &rows),
        Decision::Shown { .. }
    ));
    assert_eq!(
        decision(&curated, &rows[1], &rows),
        Decision::Shown {
            tier: Some(TierId::Local)
        }
    );
    assert_eq!(
        decision(&curated, &rows[2], &rows),
        Decision::Hidden {
            tier: Some(TierId::User)
        }
    );
    assert_eq!(
        decision(&curated, &rows[3], &rows),
        Decision::Shown {
            tier: Some(TierId::User)
        }
    );
    assert_eq!(
        decision(&curated, &rows[4], &rows),
        Decision::Shown {
            tier: Some(TierId::Local)
        }
    );
    assert_eq!(decision(&curated, &rows[5], &rows), Decision::Unmatched);
    assert!(matches!(
        curated.default_for_unmatched(),
        Decision::Hidden { .. }
    ));

    // A later broad local show cannot turn the project literal re-show into hidden.
    let (_, with_local_glob) = rules(
        user,
        "[[show]]\nharness='codex'\nmodel='gpt-6-sol-preview'\n",
        "[[show]]\nharness='codex'\nmodel='gpt-6-*'\n",
    );
    assert!(matches!(
        decision(&with_local_glob, &rows[2], &rows),
        Decision::Shown { .. }
    ));

    // A local hide, even a broad glob, can override a project literal show.
    let (_, local_hide) = rules(
        "",
        "[[show]]\nharness='codex'\nmodel='gpt-6-sol'\n",
        "[[hide]]\nharness='codex'\nmodel='gpt-6-*'\n",
    );
    assert_eq!(
        decision(&local_hide, &rows[0], &rows),
        Decision::Hidden {
            tier: Some(TierId::Local)
        }
    );
}

#[test]
fn defaults_are_explicit_or_implicit_by_reachable_tier() {
    let row = row(
        HarnessId::Claude,
        "claude-opus-5",
        Some("anthropic"),
        "claude-opus-5",
    );
    for (user, project, local, expected) in [
        ("", "", "", true),
        ("[[show]]\nharness='pi'\nmodel='x'\n", "", "", true),
        ("default='hide'\n", "", "", false),
        ("default='hide'\n", "default='show'\n", "", true),
        (
            "default='show'\n",
            "[[show]]\nharness='pi'\nmodel='x'\n",
            "",
            true,
        ),
        ("", "[[show]]\nharness='pi'\nmodel='x'\n", "", false),
        ("", "", "[[show]]\nharness='pi'\nmodel='x'\n", false),
        ("", "default='show'\n", "default='hide'\n", false),
    ] {
        let (_, rules) = rules(user, project, local);
        assert!(matches!(
            decision(&rules, &row, std::slice::from_ref(&row)),
            Decision::Unmatched
        ));
        assert_eq!(
            matches!(rules.default_for_unmatched(), Decision::Shown { .. }),
            expected
        );
    }
}

#[test]
fn inherit_false_discards_all_lower_rules_and_defaults() {
    let item = row(HarnessId::Codex, "gpt-5", Some("openai"), "gpt-5");
    let (_, local_cutoff) = rules(
        "default='hide'\n[[hide]]\nharness='codex'\nmodel='gpt-5'\n",
        "[[hide]]\nharness='codex'\nmodel='gpt-5'\n",
        "inherit=false\n",
    );
    assert_eq!(
        decision(&local_cutoff, &item, std::slice::from_ref(&item)),
        Decision::Unmatched
    );
    assert_eq!(
        local_cutoff.default_for_unmatched(),
        Decision::Shown { tier: None }
    );

    let (_, project_cutoff) = rules(
        "default='hide'\n[[hide]]\nharness='codex'\nmodel='gpt-5'\n",
        "inherit=false\n",
        "",
    );
    assert_eq!(
        decision(&project_cutoff, &item, std::slice::from_ref(&item)),
        Decision::Unmatched
    );
    assert_eq!(
        project_cutoff.default_for_unmatched(),
        Decision::Shown { tier: None }
    );
}

#[test]
fn glob_crosses_slashes_literals_normalize_and_provider_variants_match() {
    let rows = vec![
        row(
            HarnessId::Pi,
            "openai-codex/gpt-5.6-sol",
            Some("openai-codex"),
            "gpt-5.6-sol",
        ),
        row(
            HarnessId::Pi,
            "openai/gpt-5.6-sol",
            Some("openai"),
            "gpt-5.6-sol",
        ),
        row(
            HarnessId::Claude,
            "claude-opus-4-6",
            Some("anthropic"),
            "claude-opus-4-6",
        ),
        row(HarnessId::Cursor, "composer-2.5", None, "composer-2.5"),
    ];
    let (_, glob) = rules("", "[[hide]]\nharness='*'\nmodel='*-sol'\n", "");
    assert!(matches!(
        decision(&glob, &rows[0], &rows),
        Decision::Hidden { .. }
    ));
    let (_, variant) = rules(
        "",
        "[[show]]\nharness='pi'\nmodel='gpt-5.6-sol'\nprovider='OPENAI'\n[[show]]\nharness='claude'\nmodel='Claude-Opus-4.6'\n[[hide]]\nharness='cursor'\nmodel='composer-*'\nprovider='openai'\n",
        "",
    );
    assert!(matches!(
        decision(&variant, &rows[0], &rows),
        Decision::Shown { .. }
    ));
    assert!(matches!(
        decision(&variant, &rows[1], &rows),
        Decision::Shown { .. }
    ));
    assert!(matches!(
        decision(&variant, &rows[2], &rows),
        Decision::Shown { .. }
    ));
    assert_eq!(decision(&variant, &rows[3], &rows), Decision::Unmatched);
    let (_, exact) = rules(
        "",
        "[[hide]]\nharness='pi'\nmodel='openai-codex/gpt-5.6-sol'\n",
        "",
    );
    assert!(matches!(
        decision(&exact, &rows[0], &rows),
        Decision::Hidden { .. }
    ));
    assert_eq!(decision(&exact, &rows[1], &rows), Decision::Unmatched);
}

#[test]
fn declarations_require_literal_concrete_show_and_do_not_create_phantoms() {
    let possible = vec![row(
        HarnessId::Pi,
        "openai-codex/gpt-5.6-sol",
        Some("openai-codex"),
        "gpt-5.6-sol",
    )];
    let (_, rules) = rules(
        "",
        "[[show]]\nharness='pi'\nmodel='gpt-5.6-sol'\n[[show]]\nharness='*'\nmodel='ghost'\n[[show]]\nharness='pi'\nmodel='ghost*'\n[[show]]\nharness='cursor'\nmodel='composer-2.5'\n",
        "",
    );
    let view = rules.project(&possible, &HarnessScope::Unrestricted);
    assert_eq!(view.rows.len(), 2);
    assert_eq!(view.rows[0].origin, RowOrigin::Both);
    assert_eq!(view.rows[1].origin, RowOrigin::Declared);
    assert_eq!(view.rows[1].key.harness, HarnessId::Cursor);
    assert_eq!(view.rows[1].key.provider, None);
    assert!(view.diagnostics.is_empty());
}

#[test]
fn disabled_declaration_dropped_uninstalled_declaration_kept_and_hidden_show_removed() {
    let (_, rules) = rules(
        "",
        "[[show]]\nharness='cursor'\nmodel='composer-2.5'\n[[show]]\nharness='pi'\nmodel='openai/gpt-5'\n[[hide]]\nharness='pi'\nmodel='openai/gpt-5'\n",
        "",
    );
    let scope = HarnessScope::Only([HarnessId::Pi].into_iter().collect());
    let view = rules.project(&[], &scope);
    assert!(view.rows.is_empty());
    assert!(
        view.diagnostics
            .iter()
            .any(|diag| diag.contains("outside the enabled harness scope"))
    );
    assert_eq!(view.diagnostics.len(), 1);
}

#[test]
fn conflicting_provider_declarations_are_diagnosed_not_materialized() {
    let (_, rules) = rules(
        "",
        "[[show]]\nharness='pi'\nmodel='openai/gpt-5'\nprovider='anthropic'\n[[show]]\nharness='claude'\nmodel='claude-opus-5'\nprovider='openai'\n[[show]]\nharness='claude'\nmodel='openai/claude-opus-5'\n[[show]]\nharness='cursor'\nmodel='composer-2.5'\nprovider='cursor'\n",
        "",
    );
    let view = rules.project(&[], &HarnessScope::Unrestricted);
    assert!(view.rows.is_empty());
    assert_eq!(view.diagnostics.len(), 4);
    assert!(
        view.diagnostics
            .iter()
            .all(|line| line.contains("mars.curated.toml"))
    );
}

#[test]
fn duplicate_out_of_scope_declarations_emit_one_diagnostic() {
    let (_, rules) = rules(
        "[[show]]\nharness='pi'\nmodel='openai/gpt-5'\n",
        "",
        "[[show]]\nharness='pi'\nmodel='OPENAI/GPT-5'\n",
    );
    let view = rules.project(
        &[],
        &HarnessScope::Only([HarnessId::Claude].into_iter().collect()),
    );
    assert!(view.rows.is_empty());
    assert_eq!(view.diagnostics.len(), 1);
    assert!(view.diagnostics[0].contains("outside the enabled harness scope"));
}

#[test]
fn prepared_matcher_decides_after_possible_inventory_is_dropped() {
    let (_, rules) = rules("", "[[show]]\nharness='pi'\nmodel='gpt-5'\n", "");
    let key = RowKey::from(&row(HarnessId::Pi, "openai/gpt-5", Some("openai"), "gpt-5"));
    let matcher = {
        let possible = vec![row(HarnessId::Pi, "openai/gpt-5", Some("openai"), "gpt-5")];
        rules.matcher(&possible)
    };
    assert_eq!(
        matcher.decide(&key),
        Decision::Shown {
            tier: Some(TierId::Project)
        }
    );
}

#[test]
fn qualified_native_literals_resolve_to_possible_rows_without_phantoms() {
    let possible = vec![
        row(
            HarnessId::Claude,
            "claude-opus-4-6",
            Some("Anthropic"),
            "claude-opus-4-6",
        ),
        row(
            HarnessId::Codex,
            "gpt-6-sol",
            Some("OpenAI-Codex"),
            "gpt-6-sol",
        ),
    ];
    let (_, rules) = rules(
        "",
        "[[show]]\nharness='claude'\nmodel='ANTHROPIC-CLAUDE/Claude-Opus-4.6'\n[[show]]\nharness='codex'\nmodel='openai/gpt-6.sol'\n",
        "",
    );
    let view = rules.project(&possible, &HarnessScope::Unrestricted);
    assert_eq!(view.rows.len(), 2);
    assert!(view.rows.iter().all(|row| row.origin == RowOrigin::Both));
    assert_eq!(view.shown_rows().count(), 2);
    assert!(view.diagnostics.is_empty());
}

#[test]
fn qualified_native_declaration_keeps_bare_launch_id() {
    let (_, rules) = rules(
        "",
        "[[show]]\nharness='claude'\nmodel='Anthropic/Claude-Opus-5.1'\n",
        "",
    );
    let view = rules.project(&[], &HarnessScope::Unrestricted);
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].origin, RowOrigin::Declared);
    assert_eq!(view.rows[0].key.harness_model_id, "Claude-Opus-5.1");
    assert_eq!(view.rows[0].key.provider.as_deref(), Some("anthropic"));
}

#[test]
fn absent_and_empty_files_show_every_possible_row() {
    let temp = TempDir::new().unwrap();
    let none = CuratedRules::load_paths(
        &temp.path().join("user"),
        &temp.path().join("project"),
        &temp.path().join("local"),
    )
    .unwrap();
    let model = row(HarnessId::Codex, "gpt-5", Some("openai"), "gpt-5");
    let view = none.project(std::slice::from_ref(&model), &HarnessScope::Unrestricted);
    assert_eq!(view.shown_rows().count(), 1);
    assert_eq!(view.rows[0].origin, RowOrigin::Possible);
    let (_, empty) = rules("# user\n", "# project\n", "# local\n");
    assert_eq!(
        empty
            .project(&[model], &HarnessScope::Unrestricted)
            .shown_rows()
            .count(),
        1
    );
}

#[test]
fn malformed_unknown_keys_harnesses_and_unreadable_files_name_path() {
    let temp = TempDir::new().unwrap();
    let user = temp.path().join("curated.toml");
    let project = temp.path().join("mars.curated.toml");
    let local = temp.path().join("mars.curated.local.toml");
    for text in [
        "unknown=true\n",
        "[[show]]\nharness='gemini'\nmodel='x'\n",
        "[[show]]\nharness='codex'\nmodel='x'\nextra=true\n",
        "[[hide]]\nharness='codex'\nmodel=''\n",
        "[[show]\n",
    ] {
        std::fs::write(&project, text).unwrap();
        let error = CuratedRules::load_paths(&user, &project, &local)
            .unwrap_err()
            .to_string();
        assert!(error.contains("mars.curated.toml"), "{error}");
    }
    std::fs::remove_file(&project).unwrap();
    std::fs::create_dir(&user).unwrap();
    let error = CuratedRules::load_paths(&user, &project, &local)
        .unwrap_err()
        .to_string();
    assert!(error.contains("curated.toml"), "{error}");
}

#[test]
#[serial_test::serial]
fn load_uses_mars_config_dir_without_a_general_user_settings_tier() {
    let temp = TempDir::new().unwrap();
    let user_dir = temp.path().join("xdg");
    std::fs::create_dir(&user_dir).unwrap();
    std::fs::write(user_dir.join("curated.toml"), "default='hide'\n").unwrap();
    let prior = std::env::var_os("MARS_CONFIG_DIR");
    unsafe { std::env::set_var("MARS_CONFIG_DIR", &user_dir) };
    let loaded = CuratedRules::load(temp.path()).unwrap();
    match prior {
        Some(value) => unsafe { std::env::set_var("MARS_CONFIG_DIR", value) },
        None => unsafe { std::env::remove_var("MARS_CONFIG_DIR") },
    }
    assert_eq!(
        loaded.default_for_unmatched(),
        Decision::Hidden {
            tier: Some(TierId::User)
        }
    );
}
