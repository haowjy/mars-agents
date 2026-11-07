/// A bare `host/path` URL (`github.com/org/repo`) spelled with `https://`; any other URL is
/// returned as written. Reading an entry applies it, so a consumer's `mars.toml` and a dependency's
/// manifest fetch the same repository from the same text.
pub fn with_default_scheme(url: &str) -> String {
    if !url.contains("://")
        && !is_local_path(url)
        && !is_ssh_shorthand(url)
        && url.contains('/')
        && url.contains('.')
    {
        format!("https://{url}")
    } else {
        url.to_string()
    }
}

/// A filesystem path or `file:` URL, which never takes a scheme. Git's own rule: with a colon, it
/// is local only if a `/` comes before the first one (`./a:b`, `~/x:y`) or it opens with a drive
/// letter (`C:/x`, `C:\x`); so `~user@host:repo` is SSH. Without a colon, a leading `/`, `.` or `~`.
fn is_local_path(url: &str) -> bool {
    let bytes = url.as_bytes();
    if url.starts_with("file:")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return true;
    }
    match url.split_once(':') {
        Some((before_colon, _)) => before_colon.contains('/'),
        None => url.starts_with(['/', '.', '~']),
    }
}

/// Git's scp-like SSH form: `user@host:path` whatever the path (`git@host:/srv/repo.git` included),
/// or `host:path` without a user unless it is `host:<port>/…`. A `scheme://` URL or a local path is
/// not one.
pub fn is_ssh_shorthand(url: &str) -> bool {
    if url.contains("://") || is_local_path(url) {
        return false;
    }
    let Some((authority, path)) = url.split_once(':') else {
        return false;
    };
    if path.is_empty() || authority.contains(['/', '\\']) {
        return false;
    }
    match authority.split_once('@') {
        Some((user, host)) => !user.is_empty() && !host.is_empty(),
        None => {
            let (first, rest) = path.split_once('/').unwrap_or((path, ""));
            let is_port =
                !rest.is_empty() && !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit());
            !authority.is_empty() && !is_port
        }
    }
}

/// SSH and HTTPS forms share identity; ports and path case remain significant.
pub fn canonicalize_git_url(url: &str) -> String {
    let mut s = url.to_string();

    for prefix in &["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }

    // Paths can contain '@'; userinfo precedes the first '/'.
    if let Some(at_pos) = s.find('@') {
        let slash_pos = s.find('/').unwrap_or(s.len());
        if at_pos < slash_pos {
            s = s[at_pos + 1..].to_string();
        }
    }

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
                s.replace_range(colon_pos..colon_pos + 1, "/");
            }
        }
    }

    if let Some(rest) = s.strip_suffix(".git") {
        s = rest.to_string();
    }

    if let Some(rest) = s.strip_suffix('/') {
        s = rest.to_string();
    }

    // Path segments are case-sensitive on most git hosts.
    if let Some(slash_pos) = s.find('/') {
        let host = s[..slash_pos].to_ascii_lowercase();
        s = format!("{host}{}", &s[slash_pos..]);
    } else {
        s = s.to_ascii_lowercase();
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn ssh_and_https_converge() {
        let https = canonicalize_git_url("https://github.com/foo/bar");
        let ssh = canonicalize_git_url("git@github.com:foo/bar.git");
        let ssh_url = canonicalize_git_url("ssh://git@github.com/foo/bar");
        assert_eq!(https, ssh);
        assert_eq!(https, ssh_url);
    }

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

    #[test]
    fn strips_trailing_slash() {
        assert_eq!(
            canonicalize_git_url("https://github.com/foo/bar/"),
            "github.com/foo/bar"
        );
    }

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

    #[test]
    fn preserves_explicit_port_in_url_form() {
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

    #[test]
    fn scp_colon_converted_to_slash() {
        assert_eq!(
            canonicalize_git_url("git@github.com:org/repo.git"),
            "github.com/org/repo"
        );
    }

    #[test]
    fn scp_digit_leading_path_segment() {
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

    #[test]
    fn bare_host_no_path_lowercased() {
        assert_eq!(canonicalize_git_url("GITHUB.COM"), "github.com");
    }

    #[test]
    fn idempotent_on_already_canonical_form() {
        let canonical = "github.com/foo/bar";
        assert_eq!(canonicalize_git_url(canonical), canonical);
    }

    #[test]
    fn default_scheme_table() {
        let unchanged = [
            "git@host:/srv/repo.git",
            "git@host.example:org/repo.git",
            "git@host:org/repo",
            "github.com:org/repo",
            "example.com:123team/repo",
            "host:~user/repo",
            "~user@host:repo",
            "./a:b",
            "~/x:y",
            "[::1]:8080/org/repo.git",
            "localhost:8080/repo",
            "C:/x/repo.d",
            "C:\\x\\repo",
            "D:repo.git/x",
            "./rel/x.git",
            "../up/x.git",
            "/abs/x.y/repo",
            "~/x.y/repo",
            "file:/x.y/r",
            "file:///x.y/r",
            "ssh://git@github.com/org/repo",
            "https://github.com/org/repo",
            "org/repo",
        ];
        for url in unchanged {
            assert_eq!(with_default_scheme(url), url, "{url}");
        }
        assert_eq!(
            with_default_scheme("github.com/org/repo"),
            "https://github.com/org/repo"
        );
        assert_eq!(
            with_default_scheme("example.com:8080/org/repo"),
            "https://example.com:8080/org/repo"
        );
    }

    #[test]
    fn ssh_shorthand_table() {
        for url in [
            "git@host:/srv/repo.git",
            "git@host:org/repo",
            "git@host:8080/repo",
            "github.com:org/repo",
            "example.com:123team/repo",
            "host:/srv/repo.git",
            "host:~user/repo",
            "~user@host:repo",
        ] {
            assert!(is_ssh_shorthand(url), "{url}");
        }
        for url in [
            "example.com:8080/org/repo",
            "git@host:",
            "@host:org/repo",
            "ssh://git@host/org/repo",
            "/srv/repo:x",
            "./a:b",
            "~/x:y",
            "C:/x",
            "\\\\?\\C:\\x",
            "file:/x",
            "org/repo",
        ] {
            assert!(!is_ssh_shorthand(url), "{url}");
        }
    }
}
