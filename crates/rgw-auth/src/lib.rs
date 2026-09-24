//! Request authentication: the counterpart of `rgw_auth_s3.{h,cc}` and the
//! `rgw::auth` engines in `rgw_auth.h`, trimmed to AWS Signature Version 4
//! with header-based signing (presigned URLs and SigV2 are out of scope)
//! plus anonymous access.
//!
//! [`authenticate`] is an axum middleware. It verifies the signature and the
//! payload hash, decodes `aws-chunked` bodies so handlers see plain bytes,
//! and inserts an [`Identity`] request extension. Failures are rendered as
//! S3 error documents, as RGW does for every API behind the S3 auth path.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use rgw_sal::Driver;
use rgw_types::{CapPerm, UserId, UserInfo};

/// A request signed by a known access key.
#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub info: UserInfo,
    pub access_key_id: String,
}

/// `rgw::auth::Identity`, trimmed to the two kinds this spike serves.
#[derive(Clone, Debug)]
pub enum Identity {
    Anonymous,
    User(AuthedUser),
}

impl Identity {
    pub fn user(&self) -> Option<&UserInfo> {
        match self {
            Self::Anonymous => None,
            Self::User(u) => Some(&u.info),
        }
    }

    pub fn user_id(&self) -> Option<&UserId> {
        self.user().map(|u| &u.user_id)
    }

    /// `rgw::auth::Identity::is_admin_of` for the admin API: the `admin`
    /// flag short-circuits, otherwise the cap must be held.
    pub fn check_cap(&self, cap: &str, perm: CapPerm) -> bool {
        self.user().is_some_and(|u| u.admin || u.caps.check_cap(cap, perm))
    }
}

#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Allowed clock skew, `rgw_auth_max_skew` (RGW's default is 15 minutes).
    pub max_skew: chrono::Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { max_skew: chrono::Duration::minutes(15) }
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub driver: Arc<dyn Driver>,
    pub config: AuthConfig,
}

/// STUB: every request is anonymous. The auth worker replaces this body.
pub async fn authenticate(State(_state): State<AuthState>, mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(Identity::Anonymous);
    next.run(req).await
}
