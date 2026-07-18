//! Typed LLM provider identity (config / CLI / runtime).

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Kimi,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::Kimi => "kimi",
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => None,
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::DeepSeek => Some("https://api.deepseek.com"),
            Self::Kimi => Some("https://api.moonshot.cn/v1"),
        }
    }

    pub fn is_openai_compatible(self) -> bool {
        !matches!(self, Self::Anthropic)
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "deepseek" => Ok(Self::DeepSeek),
            "kimi" => Ok(Self::Kimi),
            other => Err(format!(
                "unknown provider '{other}'; expected anthropic|openai|deepseek|kimi"
            )),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn from_str_round_trip() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
        ] {
            assert_eq!(ProviderKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(ProviderKind::from_str("foo").is_err());
        assert!(ProviderKind::from_str("moonshot").is_err());
    }

    #[test]
    fn default_base_urls() {
        assert_eq!(
            ProviderKind::OpenAi.default_base_url(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            ProviderKind::DeepSeek.default_base_url(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            ProviderKind::Kimi.default_base_url(),
            Some("https://api.moonshot.cn/v1")
        );
        assert_eq!(ProviderKind::Anthropic.default_base_url(), None);
    }

    #[test]
    fn openai_compatible_flags() {
        assert!(!ProviderKind::Anthropic.is_openai_compatible());
        assert!(ProviderKind::OpenAi.is_openai_compatible());
        assert!(ProviderKind::DeepSeek.is_openai_compatible());
        assert!(ProviderKind::Kimi.is_openai_compatible());
    }
}
