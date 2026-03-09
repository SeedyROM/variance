//! Sliding-window rate limiter implemented as a Tower layer.
//! Protects the localhost HTTP API against a compromised local process flooding it.
//! State is `Arc`-shared so clones (axum clones per-connection) share one counter.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

struct WindowState {
    count: u64,
    window_start: u64,
}

/// Tower layer that enforces a sliding-window rate limit.
/// Returns 429 Too Many Requests when the limit is exceeded.
#[derive(Clone)]
pub struct LocalRateLimitLayer {
    state: Arc<Mutex<WindowState>>,
    max_requests: u64,
    window_secs: u64,
}

impl LocalRateLimitLayer {
    pub fn new(max_requests: u64, window_secs: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(WindowState {
                count: 0,
                window_start: 0,
            })),
            max_requests,
            window_secs,
        }
    }
}

impl<S> tower::Layer<S> for LocalRateLimitLayer {
    type Service = LocalRateLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LocalRateLimit {
            inner,
            state: self.state.clone(),
            max_requests: self.max_requests,
            window_secs: self.window_secs,
        }
    }
}

#[derive(Clone)]
pub struct LocalRateLimit<S> {
    inner: S,
    state: Arc<Mutex<WindowState>>,
    max_requests: u64,
    window_secs: u64,
}

impl<S> tower::Service<Request<Body>> for LocalRateLimit<S>
where
    S: tower::Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let allowed = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if now.saturating_sub(state.window_start) >= self.window_secs {
                state.window_start = now;
                state.count = 1;
                true
            } else if state.count < self.max_requests {
                state.count += 1;
                true
            } else {
                false
            }
        };

        if !allowed {
            return Box::pin(async {
                Ok((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({ "error": "Rate limit exceeded" })),
                )
                    .into_response())
            });
        }

        let future = self.inner.call(req);
        Box::pin(future)
    }
}
