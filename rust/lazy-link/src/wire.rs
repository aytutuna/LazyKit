//! The plumbing the relay and account clients share: an address, a transport to dial it with, the
//! session token to present and one HTTP request over a fresh connection.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use crate::http;
use crate::transport::{self, Transport, TransportStream};
use std::sync::Arc;
use std::time::Duration;

/// How long a connect or a plain request may take before it counts as failed.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// A larger Retry-After from the service or anything in front of it is cut to this many seconds.
const MAX_RETRY_AFTER: u64 = 3600;

// The services answer their per-address limit with 503 and a Retry-After in seconds.
pub(crate) fn retry_after(value: Option<&str>) -> Option<Duration> {
    let secs: u64 = value?.trim().parse().ok()?;
    Some(Duration::from_secs(secs.min(MAX_RETRY_AFTER)))
}

/// What a call came back with, before a client turns it into its own result.
pub(crate) struct Reply {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub reason: Option<String>,
    pub body: String,
}

#[derive(Clone)]
pub(crate) struct Wire {
    /// As written in the base address, port included when there is one: what a Host header says.
    pub authority: String,
    host: String,
    port: u16,
    pub prefix: String,
    transport: Arc<dyn Transport>,
    pub session: Option<String>,
}

impl Wire {
    /// `base` is an address up to the version, for example `https://alazy.app/relay/v1`.
    pub fn new(base: &str, transport: Arc<dyn Transport>) -> Self {
        let secure = base.starts_with("https");
        let rest = base.split_once("://").map_or(base, |(_, rest)| rest);
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse().unwrap()),
            _ => (authority, if secure { 443 } else { 80 }),
        };
        let path = path.trim_end_matches('/');
        Self {
            authority: authority.into(),
            host: host.into(),
            port,
            prefix: if path.is_empty() {
                String::new()
            } else {
                format!("/{path}")
            },
            transport,
            session: None,
        }
    }

    // The connect runs as its own task: when the caller times out and goes away, a connection
    // that finishes later still ends up here and is dropped, which closes it.
    pub async fn dial(&self) -> Result<TransportStream, String> {
        let transport = self.transport.clone();
        let host = self.host.clone();
        let port = self.port;
        tokio::spawn(async move { transport::open(&*transport, &host, port, &host).await })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.0)
    }

    /// One request to `prefix` + `path`, with the session token when there is one.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<Reply, String> {
        let path = format!("{}{path}", self.prefix);
        let request = async {
            let io = self.dial().await?;
            http::request(
                io,
                &self.authority,
                method,
                &path,
                body,
                self.session.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())
        };
        let answer = tokio::time::timeout(REQUEST_TIMEOUT, request)
            .await
            .map_err(|_| "the service did not answer in time".to_string())??;
        Ok(Reply {
            status: answer.status,
            retry_after: retry_after(answer.retry_after.as_deref()),
            reason: answer.reason,
            body: String::from_utf8_lossy(&answer.body).into_owned(),
        })
    }
}
