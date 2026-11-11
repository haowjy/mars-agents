//! The rewrite suggestions for the removed `[settings.model_visibility]` table, which moved to
//! `mars.curated.toml`.

pub(crate) fn migration(legacy: &toml::Value) -> String {
    let mut suggestions = Vec::new();
    for (key, section) in [("include", "show"), ("exclude", "hide")] {
        if let Some(patterns) = legacy.get(key).and_then(toml::Value::as_array) {
            for pattern in patterns.iter().filter_map(toml::Value::as_str) {
                let (harness, provider, model) =
                    match pattern.split('/').collect::<Vec<_>>().as_slice() {
                        [provider, model]
                            if !provider.is_empty() && *provider != "*" && !model.is_empty() =>
                        {
                            ("*", Some(*provider), *model)
                        }
                        [_, _, _, ..] => ("opencode", None, pattern),
                        _ => ("*", None, pattern),
                    };
                let provider = provider
                    .map(|value| format!(" provider={}", toml::Value::String(value.into())))
                    .unwrap_or_default();
                suggestions.push(format!(
                    "  {key} = {} → [[{section}]] harness={}{} model={}",
                    toml::Value::String(pattern.into()),
                    toml::Value::String(harness.into()),
                    provider,
                    toml::Value::String(model.into())
                ));
            }
        }
    }
    if let Some(providers) = legacy.get("providers").and_then(toml::Value::as_array) {
        for provider in providers.iter().filter_map(toml::Value::as_str) {
            suggestions.push(format!(
                "  providers = {} → [[show]] harness=\"*\" provider={} model=\"*\"",
                toml::Value::String(provider.into()),
                toml::Value::String(provider.into())
            ));
        }
    }
    if suggestions.is_empty() {
        "  Translate include to [[show]], exclude to [[hide]], and providers to [[show]].".into()
    } else {
        suggestions.join("\n")
    }
}
