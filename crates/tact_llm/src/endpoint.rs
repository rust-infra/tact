//! Canonical provider endpoints and fixed protocol suffix joining.

use std::fmt;

/// An opaque canonical HTTP(S) endpoint.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Endpoint(String);

impl Endpoint {
    pub(crate) fn parse(raw: &str) -> Result<Self, EndpointError> {
        let mut url = url::Url::parse(raw).map_err(|_| EndpointError::InvalidUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(EndpointError::UnsupportedScheme);
        }
        if url.host_str().is_none() {
            return Err(EndpointError::MissingHost);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::EmbeddedCredentials);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(EndpointError::QueryOrFragmentNotAllowed);
        }

        let path = match url.path() {
            "" => "/".to_string(),
            "/" => "/".to_string(),
            path if path.ends_with('/') => path.strip_suffix('/').unwrap_or(path).to_string(),
            path => path.to_string(),
        };
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        Ok(Self(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compare a raw configured endpoint after canonicalization.
    pub fn matches(&self, raw: &str) -> bool {
        Self::parse(raw).is_ok_and(|candidate| candidate == *self)
    }

    pub(crate) fn dispatch_base(&self) -> &str {
        self.0.trim_end_matches('/')
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Endpoint").field(&self.0).finish()
    }
}

/// Endpoint parsing failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    #[error("endpoint is not a valid URL")]
    InvalidUrl,
    #[error("endpoint scheme must be http or https")]
    UnsupportedScheme,
    #[error("endpoint is missing a host")]
    MissingHost,
    #[error("endpoint must not contain embedded credentials")]
    EmbeddedCredentials,
    #[error("endpoint must not contain a query or fragment")]
    QueryOrFragmentNotAllowed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_and_joins_root_endpoints() {
        let endpoint = Endpoint::parse("HTTPS://API.Example.com:443/").unwrap();
        assert_eq!(endpoint.as_str(), "https://api.example.com/");
        assert_eq!(
            format!("{}/messages", endpoint.dispatch_base()),
            "https://api.example.com/messages"
        );
    }

    #[test]
    fn rejects_unsafe_endpoint_parts() {
        assert_eq!(
            Endpoint::parse("https://user:pass@example.com"),
            Err(EndpointError::EmbeddedCredentials)
        );
        assert_eq!(
            Endpoint::parse("https://example.com/?x=1"),
            Err(EndpointError::QueryOrFragmentNotAllowed)
        );
        assert_eq!(
            Endpoint::parse("ftp://example.com"),
            Err(EndpointError::UnsupportedScheme)
        );
    }

    #[test]
    fn removes_only_one_non_root_trailing_slash() {
        let endpoint = Endpoint::parse("https://example.com/v1//").unwrap();
        assert_eq!(endpoint.as_str(), "https://example.com/v1/");
    }
}
