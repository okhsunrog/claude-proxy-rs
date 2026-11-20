//! Client contract and cache policy selection. Explicit request headers override
//! configured defaults; Auto uses client identity only as a mapping hint. Tool
//! schema checks still decide whether an individual alias is safe to adapt.
use axum::http::HeaderMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClientProfile {
    #[default]
    Auto,
    Generic,
    OpenAi,
    Cline,
    Roo,
    OpenCode,
    Native,
}

impl ClientProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "generic" => Ok(Self::Generic),
            "openai" => Ok(Self::OpenAi),
            "cline" => Ok(Self::Cline),
            "roo" => Ok(Self::Roo),
            "opencode" => Ok(Self::OpenCode),
            "claude-code" => Ok(Self::Native),
            _ => Err(
                "Expected client profile: auto, generic, openai, cline, roo, opencode, claude-code"
                    .into(),
            ),
        }
    }

    pub fn resolve(self, headers: &HeaderMap) -> Self {
        if self != Self::Auto {
            return self;
        }
        let identity = ["user-agent", "x-title"]
            .iter()
            .filter_map(|name| headers.get(*name)?.to_str().ok())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if identity.starts_with("claude-cli/") {
            Self::Native
        } else if identity.contains("opencode") {
            Self::OpenCode
        } else if identity.contains("roo-code")
            || identity.contains("roo code")
            || identity.contains("roo-cline")
        {
            Self::Roo
        } else if identity.contains("cline") {
            Self::Cline
        } else if identity.contains("openai") {
            Self::OpenAi
        } else {
            Self::Generic
        }
    }

    pub fn permits_alias(self, name: &str, native: &str) -> bool {
        if name == native || name == format!("mcp_{native}") {
            return true;
        }
        match self {
            Self::OpenAi => matches!(
                name,
                "read_file" | "write_file" | "edit_file" | "execute_command"
            ),
            Self::Cline => matches!(
                name,
                "read_file" | "write_to_file" | "replace_in_file" | "execute_command"
            ),
            Self::Roo => matches!(
                name,
                "read_file" | "write_to_file" | "edit" | "search_replace" | "execute_command"
            ),
            Self::OpenCode => matches!(name, "read" | "write" | "edit" | "bash" | "shell"),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CachePolicy {
    #[default]
    Auto5m,
    Auto1h,
    Preserve,
}

impl CachePolicy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "auto-5m" => Ok(Self::Auto5m),
            "auto-1h" => Ok(Self::Auto1h),
            "preserve" => Ok(Self::Preserve),
            _ => Err("Expected cache policy: preserve, auto-5m, auto-1h".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Compatibility {
    pub profile: ClientProfile,
    pub cache: CachePolicy,
}

impl Compatibility {
    pub fn for_request(self, headers: &HeaderMap) -> Result<Self, String> {
        let read = |name| {
            headers
                .get(name)
                .map(|h| {
                    h.to_str()
                        .map_err(|error| format!("Invalid {name}: {error}"))
                })
                .transpose()
        };
        let profile = read("x-proxy-client-profile")?
            .map(ClientProfile::parse)
            .transpose()?
            .unwrap_or(self.profile)
            .resolve(headers);
        let cache = read("x-proxy-cache-policy")?
            .map(CachePolicy::parse)
            .transpose()?
            .unwrap_or(self.cache);
        Ok(Self { profile, cache })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detection_overrides_and_invalid_values() {
        for (ua, expected) in [
            ("claude-cli/2.1.326", ClientProfile::Native),
            ("opencode/2.2.7", ClientProfile::OpenCode),
            ("roo-cline/3.54", ClientProfile::Roo),
            ("Cline/1", ClientProfile::Cline),
            ("OpenAI/Python", ClientProfile::OpenAi),
            ("unknown", ClientProfile::Generic),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("user-agent", ua.parse().unwrap());
            assert_eq!(
                Compatibility::default()
                    .for_request(&headers)
                    .unwrap()
                    .profile,
                expected
            );
            headers.insert("x-proxy-client-profile", "generic".parse().unwrap());
            headers.insert("x-proxy-cache-policy", "auto-1h".parse().unwrap());
            let policy = Compatibility::default().for_request(&headers).unwrap();
            assert_eq!(policy.profile, ClientProfile::Generic);
            assert_eq!(policy.cache, CachePolicy::Auto1h);
            headers.insert("x-proxy-cache-policy", "bad".parse().unwrap());
            Compatibility::default().for_request(&headers).unwrap_err();
        }
    }
}
