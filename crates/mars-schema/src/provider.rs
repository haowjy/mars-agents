//! Provider names, the hosts that supply skills Mars installs outside the dependency graph.
//!
//! A provider is the name a host program gives what it provides (`meridian`), and it is its own
//! namespace: separate from dependency keys, dependency-declared refs must opt in with
//! `[package] uses-providers`. Skills it provides are named `<skill>__<provider>`, so a ref's
//! validity depends only on the package, never on what a machine happens to have installed.

/// The provider-name grammar: `[a-z0-9][a-z0-9-]*`.
///
/// Excludes `__`, `@` and `=` so a provider name can never be confused with a skill name's
/// separator or the `mars provide` spec syntax.
pub fn is_provider_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The provider a skill name belongs to: the suffix after its last `__`.
///
/// Returns `None` for an unprefixed name (`notify`) and for a name whose suffix is not a provider
/// name (`notify__Not-Provider`), so an author cannot claim a provider ref by spelling alone.
pub fn provider_of(skill_name: &str) -> Option<&str> {
    let (stem, provider) = skill_name.rsplit_once("__")?;
    if stem.is_empty() || !is_provider_name(provider) {
        return None;
    }
    Some(provider)
}

/// The provider a ref is deferred to, when the requesting package declares it.
///
/// The package-only half of a ref's validity: `<x>__<p>` is valid wherever `p` supplies it when
/// the package lists `p` in `[package] uses-providers`. Selection and `mars check` ask only
/// this question, so a package resolves the same on every machine; sync's validator is the one
/// reporter that also knows what this machine installed.
///
/// `declared` is the requesting package's `uses-providers`: `Some` for a package (a dependency,
/// or a package repository's own items), `None` for a consumer project's local items, which need
/// no declaration.
pub fn deferred_provider<'a>(name: &'a str, declared: Option<&[String]>) -> Option<&'a str> {
    let provider = provider_of(name)?;
    declared?
        .iter()
        .any(|declared| declared == provider)
        .then_some(provider)
}

/// The hint for a `<skill>__<provider>` ref that no package ships and no package declares.
///
/// A name can look exactly like a host provider's and still be a typo, so the hint is
/// conditional. `mars check` and selection's missing-skill report print this one text.
pub fn undeclared_provider_hint(provider: &str) -> String {
    format!(
        "if `{provider}` is a host that provides this skill, declare it in `[package] uses-providers`; otherwise check the name"
    )
}
