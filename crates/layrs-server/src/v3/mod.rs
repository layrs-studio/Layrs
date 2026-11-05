//! Thin, fail-closed HTTP boundary for the native Sync V3 protocol.
//!
//! This module deliberately owns no SQL or repository policy. Handlers decode
//! bounded wire requests and delegate to an injected [`V3Facade`].

mod adapters;
mod auth;
mod body;
mod endpoints;
mod error;
mod production;
mod range;
mod router;
mod state;

pub use production::ProductionV3Facade;
pub use range::{ByteRange, RequestedRange, parse_range_header, resolve_range};
pub use router::router;
pub use state::{
    AuthenticatedPrincipal, DownloadResult, DownloadStream, StoredTransactionReceipt,
    UnavailableV3Facade, UploadDisposition, UploadResult, UploadStream, V3Facade, V3HttpLimits,
    V3HttpState, V3ServiceError,
};

use std::{net::SocketAddr, sync::Arc};

#[derive(Clone, Debug)]
pub struct V3ServerConfig {
    pub addr: String,
}

#[derive(Debug)]
pub struct V3ServerError(String);

impl std::fmt::Display for V3ServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for V3ServerError {}

pub async fn serve(config: V3ServerConfig, facade: Arc<dyn V3Facade>) -> Result<(), V3ServerError> {
    let address = config
        .addr
        .parse::<SocketAddr>()
        .map_err(|error| V3ServerError(format!("invalid server address: {error}")))?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| V3ServerError(format!("could not bind server: {error}")))?;
    let app = router(V3HttpState::new(facade, V3HttpLimits::default()));
    axum::serve(listener, app)
        .await
        .map_err(|error| V3ServerError(format!("server failed: {error}")))
}

#[cfg(test)]
mod tests;
