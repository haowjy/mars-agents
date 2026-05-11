/// Canonicalize a git URL for identity comparison and cache keying.
///
/// SSH and HTTPS forms of the same repo produce the same canonical form.
///
/// Canonical form: `host/path` (lowercase host, no protocol, no `.git`,
/// no trailing slash, no userinfo, SCP colon converted to slash).
///
/// Port handling: Explicit ports in URL-style forms (`host:port/path`) are
/// preserved. SCP-style colon (`host:path`) is distinguished by checking
/// whether the text between the colon and the next `/` (or end of string) is
/// **entirely digits** (a real port) vs. a path segment (convert to slash).
/// This correctly handles digit-leading path segments like `123team/repo`.
///
/// # Examples
///
/// ```
/// # use mars_agents::source::canonical::canonicalize_git_url;
/// assert_eq!(canonicalize_git_url("https://github.com/foo/bar"),   "github.com/foo/bar");
/// assert_eq!(canonicalize_git_url("git@github.com:foo/bar.git"),   "github.com/foo/bar");
/// assert_eq!(canonicalize_git_url("ssh://git@github.com/foo/bar"), "github.com/foo/bar");
/// assert_eq!(canonicalize_git_url("GITHUB.COM/Foo/Bar"),           "github.com/Foo/Bar");
/// ```
pub fn canonicalize_git_url(url: &str) -> String {
    let mut s = url.to_string();

    // 1. Strip protocol prefixes
    for prefix in &["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }

    // 2. Strip userinfo (git@, user:pass@, etc.)
    //    Only strip if the '@' comes before the first '/' (it's a userinfo,
    //    not part of the path).
    if let Some(at_pos) = s.find('@') {
        let slash_pos = s.find('/').unwrap_or(s.len());
        if at_pos < slash_pos {
            s = s[at_pos + 1..].to_string();
        }
    }

    // 3. Handle SCP-style colon vs port colon.
    //    After stripping userinfo we may have:
    //      - `github.com:foo/bar`    (SCP – colon is a separator, convert to /)
    //      - `github.com:123team/r`  (SCP – digit-leading path, convert to /)
    //      - `github.com:1234/path`  (URL with port – keep the colon)
    //      - `github.com/foo/bar`    (already URL-style – no colon)
    if let Some(colon_pos) = s.find(':') {
        let before_colon = &s[..colon_pos];
        let after_colon = &s[colon_pos + 1..];
        // Only treat as a port when all characters up to the next '/' (or end of
        // string) are ASCII digits.  A digit-leading path segment like `123team`
        // must NOT be treated as a port.
        if !before_colon.contains('/') && !after_colon.starts_with("//") {
            let port_candidate = after_colon.split('/').next().unwrap_or("");
            let is_port =
                !port_candidate.is_empty() && port_candidate.chars().all(|c| c.is_ascii_digit());
            if !is_port {
                // SCP-style colon: convert to slash
                s.replace_range(colon_pos..colon_pos + 1, "/");
            }
        }
    }

    // 4. Strip trailing `.git` suffix
    if let Some(rest) = s.strip_suffix(".git") {
        s = rest.to_string();
    }

    // 5. Strip trailing slash
    if let Some(rest) = s.strip_suffix('/') {
        s = rest.to_string();
    }

    // 6. Lowercase the host portion only (everything before the first `/`).
    //    Path segments are case-sensitive on most git hosts.
    if let Some(slash_pos) = s.find('/') {
        let host = s[..slash_pos].to_ascii_lowercase();
        s = format!("{host}{}", &s[slash_pos..]);
    } else {
        // No slash means the whole string is a host (bare domain URL).
        s = s.to_ascii_lowercase();
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Protocol forms ──────────────────────────────────────────────────────

    #[test]
    fn https_form() {
        assert_eq!(
            canonicalize_git_url("https://github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn http_form() {
        assert_eq!(
            canonicalize_git_url("http://gitlab.com/org/repo"),
            "gitlab.com/org/repo"
        );
    }

    #[test]
    fn ssh_url_form() {
        assert_eq!(
            canonicalize_git_url("ssh://git@github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn git_protocol_form() {
        assert_eq!(
            canonicalize_git_url("git://github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn scp_form_git_at() {
        assert_eq!(
            canonicalize_git_url("git@github.com:foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn bare_domain_form() {
        assert_eq!(
            canonicalize_git_url("github.com/meridian-flow/meridian-base"),
            "github.com/meridian-flow/meridian-base"
        );
    }

    // ── All three protocol forms for the same repo converge ────────────────

    #[test]
    fn ssh_and_https_converge() {
        let https = canonicalize_git_url("https://github.com/foo/bar");
        let ssh = canonicalize_git_url("git@github.com:foo/bar.git");
        let ssh_url = canonicalize_git_url("ssh://git@github.com/foo/bar");
        assert_eq!(https, ssh);
        assert_eq!(https, ssh_url);
    }

    // ── `.git` suffix tolerance ─────────────────────────────────────────────

    #[test]
    fn strips_git_suffix_https() {
        assert_eq!(
            canonicalize_git_url("https://github.com/foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn strips_git_suffix_bare() {
        assert_eq!(
            canonicalize_git_url("github.com/foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    // ── Trailing slash tolerance ────────────────────────────────────────────

    #[test]
    fn strips_trailing_slash() {
        assert_eq!(
            canonicalize_git_url("https://github.com/foo/bar/"),
            "github.com/foo/bar"
        );
    }

    // ── Lowercase host, case-preserved path ────────────────────────────────

    #[test]
    fn lowercases_host_only() {
        assert_eq!(
            canonicalize_git_url("GITHUB.COM/Foo/Bar"),
            "github.com/Foo/Bar"
        );
    }

    #[test]
    fn lowercases_host_in_https_url() {
        assert_eq!(
            canonicalize_git_url("https://GITHUB.COM/Foo/Bar"),
            "github.com/Foo/Bar"
        );
    }

    // ── Userinfo stripped ──────────────────────────────────────────────────

    #[test]
    fn strips_userinfo_user_password() {
        assert_eq!(
            canonicalize_git_url("https://user:pass@github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn strips_userinfo_bare_user() {
        assert_eq!(
            canonicalize_git_url("https://user@github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }

    // ── Port preservation ──────────────────────────────────────────────────

    #[test]
    fn preserves_explicit_port_in_url_form() {
        // A URL-style port (digit after colon) is kept as-is.
        assert_eq!(
            canonicalize_git_url("git://gitlab.localtest.me:19424/group/pkg.git"),
            "gitlab.localtest.me:19424/group/pkg"
        );
    }

    #[test]
    fn preserves_explicit_port_in_https() {
        assert_eq!(
            canonicalize_git_url("https://git.example.com:8443/org/repo"),
            "git.example.com:8443/org/repo"
        );
    }

    // ── SCP colon becomes slash, port digit is preserved ───────────────────

    #[test]
    fn scp_colon_converted_to_slash() {
        assert_eq!(
            canonicalize_git_url("git@github.com:org/repo.git"),
            "github.com/org/repo"
        );
    }

    // ── SCP path with digit-leading segment (regression: was mis-detected as port)

    #[test]
    fn scp_digit_leading_path_segment() {
        // "123team" starts with a digit but is NOT a port — must become a slash.
        assert_eq!(
            canonicalize_git_url("git@example.com:123team/repo.git"),
            "example.com/123team/repo"
        );
    }

    #[test]
    fn scp_digit_leading_path_segment_no_git_suffix() {
        assert_eq!(
            canonicalize_git_url("git@example.com:9front/repo"),
            "example.com/9front/repo"
        );
    }

    // ── Real ports are preserved even when path follows ────────────────────

    #[test]
    fn port_only_digits_is_preserved_https() {
        assert_eq!(
            canonicalize_git_url("https://gitlab.com:8443/org/repo"),
            "gitlab.com:8443/org/repo"
        );
    }

    #[test]
    fn port_only_digits_is_preserved_git_protocol() {
        assert_eq!(
            canonicalize_git_url("git://custom.host:19424/group/pkg.git"),
            "custom.host:19424/group/pkg"
        );
    }

    // ── Subgroup / nested paths preserved ──────────────────────────────────

    #[test]
    fn subgroup_path_preserved() {
        assert_eq!(
            canonicalize_git_url("https://gitlab.com/group/subgroup/repo"),
            "gitlab.com/group/subgroup/repo"
        );
    }

    #[test]
    fn subgroup_path_ssh_preserved() {
        assert_eq!(
            canonicalize_git_url("git@gitlab.com:group/subgroup/repo.git"),
            "gitlab.com/group/subgroup/repo"
        );
    }

    // ── No-slash bare host ─────────────────────────────────────────────────

    #[test]
    fn bare_host_no_path_lowercased() {
        assert_eq!(canonicalize_git_url("GITHUB.COM"), "github.com");
    }

    // ── Idempotence ────────────────────────────────────────────────────────

    #[test]
    fn idempotent_on_already_canonical_form() {
        let canonical = "github.com/foo/bar";
        assert_eq!(canonicalize_git_url(canonical), canonical);
    }
}
