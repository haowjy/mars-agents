//! Shell network policy: the mode a profile, overlay, settings or CLI flag names, and the host
//! grammar of its allow and deny sets.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Shell network mode. Unset follows the sandbox, which Meridian decides, not Mars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicy {
    Allow,
    Deny,
    Allowlist,
}

impl NetworkPolicy {
    /// The values `parse` accepts, as the invalid-value errors list them.
    pub const ALLOWED: &'static str = "allow, deny, allowlist";

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "allowlist" => Some(Self::Allowlist),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Allowlist => "allowlist",
        }
    }
}

impl FromStr for NetworkPolicy {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| {
            format!(
                "invalid network policy `{value}`; expected one of: {}",
                Self::ALLOWED
            )
        })
    }
}

impl<'de> Deserialize<'de> for NetworkPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

/// A canonical network host or Codex-compatible host wildcard.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct NetworkHost(String);

impl NetworkHost {
    pub const ALLOWED: &'static str = "exact host, IP address, *.example.com (subdomains), or **.example.com (apex and subdomains); internationalized names must use punycode; URLs, ports, paths, numeric or hex-looking DNS endings, single-label wildcards, empty strings, and bare * are not allowed";

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetworkHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for NetworkHost {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.is_empty() {
            return Err(invalid_host(raw));
        }
        if raw.trim() != raw {
            return Err(invalid_host(raw));
        }

        let canonical = raw.to_ascii_lowercase();
        let canonical = canonical.strip_suffix('.').unwrap_or(&canonical);
        if canonical.is_empty()
            || canonical == "*"
            || canonical.contains("//")
            || canonical.contains('/')
            || canonical.contains('?')
            || canonical.contains('#')
        {
            return Err(invalid_host(raw));
        }

        if let Ok(ip) = canonical.parse::<IpAddr>() {
            let ip = match ip {
                IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map_or(IpAddr::V6(ipv6), IpAddr::V4),
                ipv4 => ipv4,
            };
            return Ok(Self(ip.to_string()));
        }
        if canonical.contains(':') {
            return Err(invalid_host(raw));
        }

        let (prefix, host) = if let Some(host) = canonical.strip_prefix("**.") {
            ("**.", host)
        } else if let Some(host) = canonical.strip_prefix("*.") {
            ("*.", host)
        } else {
            ("", canonical)
        };
        if host.contains('*')
            || (!prefix.is_empty() && host.parse::<IpAddr>().is_ok())
            || (!prefix.is_empty() && !host.contains('.'))
            || !valid_dns_host(host)
            || host.rsplit('.').next().is_some_and(numeric_or_hex_label)
        {
            return Err(invalid_host(raw));
        }
        Ok(Self(format!("{prefix}{host}")))
    }
}

fn numeric_or_hex_label(label: &str) -> bool {
    label.bytes().all(|byte| byte.is_ascii_digit())
        || label.strip_prefix("0x").is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit())
        })
}

impl<'de> Deserialize<'de> for NetworkHost {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

fn valid_dns_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.is_ascii()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn invalid_host(raw: &str) -> String {
    format!(
        "invalid network host `{raw}`; expected {}",
        NetworkHost::ALLOWED
    )
}
