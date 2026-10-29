//! Fakes for tests of the engine seam. Compiled into the library so module crates'
//! integration tests can use them; they never touch the network or the disk.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::http::{HttpError, HttpRequest, HttpResponse, Transport};

/// Serves queued responses (404 when the queue is empty) and records every request.
#[derive(Clone, Default)]
pub struct RecordingTransport {
    inner: Arc<Mutex<Recorded>>,
}

#[derive(Default)]
struct Recorded {
    requests: Vec<HttpRequest>,
    responses: VecDeque<Result<HttpResponse, HttpError>>,
}

impl RecordingTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_response(&self, response: Result<HttpResponse, HttpError>) {
        self.lock().responses.push_back(response);
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.lock().requests.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Transport for RecordingTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut inner = self.lock();
        inner.requests.push(request.clone());
        inner.responses.pop_front().unwrap_or(Ok(HttpResponse {
            status: 404,
            headers: Vec::new(),
            body: Vec::new(),
        }))
    }
}
