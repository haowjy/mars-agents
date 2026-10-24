use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::frontmatter::Frontmatter;
use crate::permission_set::PermissionSet;

use super::looks_like_network_key;

/// Shell network mode authored on profiles, overlays, settings, policies, and the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicy {
    Allow,
    Deny,
    Allowlist,
}

impl NetworkPolicy {
    pub const ALLOWED: &'static str = "allow, deny, allowlist";

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
        match value {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            "allowlist" => Ok(Self::Allowlist),
            _ => Err(format!(
                "invalid network policy `{value}`; expected one of: {}",
                Self::ALLOWED
            )),
        }
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

pub(crate) struct AgentNetworkFields {
    pub(crate) policy: Option<NetworkPolicy>,
    pub(crate) allow: PermissionSet<NetworkHost>,
    pub(crate) deny: PermissionSet<NetworkHost>,
}

pub(crate) enum AgentNetworkDiagnostic {
    InvalidFieldValue {
        field: String,
        value: String,
        allowed: &'static str,
    },
    DuplicateField {
        field: String,
        alias: String,
    },
    UnknownField {
        field: String,
    },
}

pub(crate) fn parse_agent_network(
    fm: &Frontmatter,
) -> (AgentNetworkFields, Vec<AgentNetworkDiagnostic>) {
    let mut diags = Vec::new();
    let policy = fm.get("network").and_then(|value| {
        let Some(value) = value.as_str() else {
            diags.push(AgentNetworkDiagnostic::InvalidFieldValue {
                field: "network".to_string(),
                value: serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}")),
                allowed: NetworkPolicy::ALLOWED,
            });
            return None;
        };
        match value.parse::<NetworkPolicy>() {
            Ok(network) => Some(network),
            Err(_) => {
                diags.push(AgentNetworkDiagnostic::InvalidFieldValue {
                    field: "network".to_string(),
                    value: value.to_string(),
                    allowed: NetworkPolicy::ALLOWED,
                });
                None
            }
        }
    });
    let allow_value = find_aliased_network_field(fm, "network-allow", "network_allow", &mut diags);
    let allow = parse_network_hosts("network-allow", allow_value, true, &mut diags);
    let deny_value = find_aliased_network_field(fm, "network-deny", "network_deny", &mut diags);
    let deny = parse_network_hosts("network-deny", deny_value, false, &mut diags);

    for field in fm.keys().into_iter().filter(|field| {
        looks_like_network_key(field)
            && !matches!(
                field.as_str(),
                "network" | "network-allow" | "network_allow" | "network-deny" | "network_deny"
            )
    }) {
        diags.push(AgentNetworkDiagnostic::UnknownField { field });
    }

    (
        AgentNetworkFields {
            policy,
            allow,
            deny,
        },
        diags,
    )
}

fn find_aliased_network_field<'a>(
    fm: &'a Frontmatter,
    field: &str,
    alias: &str,
    diags: &mut Vec<AgentNetworkDiagnostic>,
) -> Option<&'a Value> {
    let value = fm.get(field);
    let alias_value = fm.get(alias);
    if value.is_some() && alias_value.is_some() {
        diags.push(AgentNetworkDiagnostic::DuplicateField {
            field: field.to_string(),
            alias: alias.to_string(),
        });
    }
    value.or(alias_value)
}

fn parse_network_hosts(
    field: &str,
    value: Option<&Value>,
    allow_replace: bool,
    diags: &mut Vec<AgentNetworkDiagnostic>,
) -> PermissionSet<NetworkHost> {
    let Some(value) = value else {
        return PermissionSet::default();
    };
    let parsed = match serde_yaml::from_value::<PermissionSet<NetworkHost>>(value.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            let value = serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"));
            diags.push(AgentNetworkDiagnostic::InvalidFieldValue {
                field: field.to_string(),
                value: format!("{value} ({error})"),
                allowed: "list of exact hosts, IPs, *.example.com, or **.example.com; or {replace: [...]} for an allow set",
            });
            return PermissionSet::default();
        }
    };
    if !allow_replace && parsed.replaces() {
        diags.push(AgentNetworkDiagnostic::InvalidFieldValue {
            field: field.to_string(),
            value: serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}")),
            allowed: "plain list; package profiles cannot replace a deny set",
        });
        return PermissionSet::default();
    }
    parsed
}
