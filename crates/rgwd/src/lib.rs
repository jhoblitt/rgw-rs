//! The gateway daemon: the counterpart of `rgw_main.cc` / `rgw_appmain.cc`
//! (wiring the REST managers to the frontend) and the asio frontend in
//! `rgw_asio_frontend.cc`, here provided by tokio and axum.
//!
//! The router is a library function so integration tests can host it
//! in-process on an ephemeral port.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use rgw_auth::{AuthConfig, AuthState};
use rgw_rest_admin::AdminState;
use rgw_rest_s3::S3State;
use rgw_sal::Driver;
use tower_http::trace::TraceLayer;

/// `rgw_admin_entry`'s default.
pub const DEFAULT_ADMIN_ENTRY: &str = "admin";

/// Assemble the request pipeline. Layers apply outermost-last, so a request
/// passes tracing, then the request id, then authentication, then routing.
pub fn build_app(driver: Arc<dyn Driver>, admin_entry: &str) -> Router {
    let auth = AuthState { driver: driver.clone(), config: AuthConfig::default() };
    Router::new()
        .nest(&format!("/{admin_entry}"), rgw_rest_admin::router(AdminState { driver: driver.clone() }))
        .merge(rgw_rest_s3::router(S3State { driver }))
        .layer(from_fn_with_state(auth, rgw_auth::authenticate))
        .layer(from_fn(rgw_rest::request_id_layer))
        .layer(TraceLayer::new_for_http())
}

/// Bind and serve until the task is cancelled.
pub async fn serve(app: Router, listen: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "rgwd listening");
    axum::serve(listener, app).await?;
    Ok(())
}
