//! The network boundary. Modules reach the network only through a [`HostClient`], which
//! knows the module's allowed hosts; the [`Transport`] behind it does the actual I/O.

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
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
