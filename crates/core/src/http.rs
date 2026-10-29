//! The network boundary. Modules reach the network only through a [`HostClient`], which
//! knows the module's allowed hosts; the [`Transport`] behind it does the actual I/O.

use std::sync::Arc;
use std::time::Duration;

/// Header names whose values are credentials; never shown by `Debug`.
const SECRET_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-api-key",
];

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let secret = SECRET_HEADERS.contains(&k.to_ascii_lowercase().as_str());
                (k.as_str(), if secret { "<redacted>" } else { v.as_str() })
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The first value of header `name`, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// The URL's host is not on the module's list; the request was not sent.
    HostNotAllowed(String),
    InvalidUrl,
    /// The transport could not complete the request.
    Transport(String),
}

/// Performs HTTP requests. The real one lives in core only; tests use fakes.
pub trait Transport: Send + Sync {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// A transport that never connects, for builds without a network-backed module.
pub struct NoNetwork;

impl Transport for NoNetwork {
    fn execute(&self, _request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Transport("networking is not available".into()))
    }
}

/// The real transport: blocking ureq on the calling (scheduler) thread, TLS through
/// Windows SChannel with the system certificate store, so corporate roots work.
/// Redirects are returned, never followed, so a redirect cannot leave the allowlist.
pub struct UreqTransport {
    agent: ureq::Agent,
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_LIMIT: u64 = 1024 * 1024;

impl UreqTransport {
    /// Uses the proxy from `HTTPS_PROXY`/`ALL_PROXY`, else the Windows (WinINet) static
    /// proxy of the current user. PAC scripts and auto-detection are not supported.
    pub fn new() -> Self {
        Self::with_proxy(ureq::Proxy::try_from_env())
    }

    /// Connects directly, ignoring any proxy setting.
    pub fn without_proxy() -> Self {
        Self::with_proxy(None)
    }

    fn with_proxy(proxy: Option<ureq::Proxy>) -> Self {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        if let Some(p) = &proxy {
            log::info!("using proxy {}:{}", p.host(), p.port());
        }
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .proxy(proxy)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("UsageQuotaTray/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for UreqTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        if request.method != "GET" {
            return Err(HttpError::Transport(format!(
                "method {} is not supported",
                request.method
            )));
        }
        let mut builder = self.agent.get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        // ureq's error text names the host and the failure, never header values.
        let mut response = builder
            .call()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        Ok(HttpResponse {
            status: response.status().as_u16(),
            headers,
            body,
        })
    }
}

/// The HTTP client handed to one module's Sources.
pub struct HostClient {
    allowed_hosts: Vec<String>,
    transport: Arc<dyn Transport>,
}

impl HostClient {
    pub fn new(allowed_hosts: &[&str], transport: Arc<dyn Transport>) -> Self {
        Self {
            allowed_hosts: allowed_hosts
                .iter()
                .map(|h| h.to_ascii_lowercase())
                .collect(),
            transport,
        }
    }

    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, HttpError> {
        let host = https_host(url).ok_or(HttpError::InvalidUrl)?;
        if !self.allowed_hosts.contains(&host) {
            return Err(HttpError::HostNotAllowed(host));
        }
        self.transport.execute(&HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        })
    }
}

/// The lower-cased host of an `https://` URL, without port or userinfo.
fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingTransport;

    #[test]
    fn requests_to_undeclared_hosts_never_reach_the_transport() {
        let transport = RecordingTransport::new();
        let client = HostClient::new(&["api.example.com"], Arc::new(transport.clone()));
        for url in [
            "https://evil.example.org/x",
            "https://api.example.com.evil.org/x",
            "https://user@api.example.com/x",
            "http://api.example.com/x",
        ] {
            assert!(client.get(url, &[]).is_err(), "{url}");
        }
        assert!(transport.requests().is_empty());

        client
            .get("https://API.example.com:443/usage?x=1", &[])
            .ok();
        assert_eq!(transport.requests().len(), 1);
    }
}
