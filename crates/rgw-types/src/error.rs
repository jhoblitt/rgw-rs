//! `rgw_err` and the S3 error table (`rgw_http_s3_errors` in
//! `rgw_common.cc`), trimmed to the codes this spike can produce.
//!
//! RGW carries errors as negative errno-style ints (`-ERR_NO_SUCH_BUCKET`)
//! and maps them to an S3 code plus HTTP status at response time. Here the
//! enum is the code; the status and the wire name hang off it.

/// One error, named by its S3 `<Code>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RgwError {
    #[error("access denied")]
    AccessDenied,
    #[error("invalid access key id")]
    InvalidAccessKeyId,
    #[error("signature does not match")]
    SignatureDoesNotMatch,
    #[error("request time too skewed")]
    RequestTimeTooSkewed,
    #[error("authorization header malformed: {0}")]
    AuthorizationHeaderMalformed(String),
    #[error("x-amz-content-sha256 does not match the payload")]
    XAmzContentSha256Mismatch,
    #[error("content-md5 does not match the payload")]
    BadDigest,
    #[error("no such user")]
    NoSuchUser,
    #[error("user already exists")]
    UserAlreadyExists,
    #[error("access key already exists")]
    KeyExists,
    #[error("no such bucket")]
    NoSuchBucket,
    #[error("bucket already exists")]
    BucketAlreadyExists,
    #[error("bucket not empty")]
    BucketNotEmpty,
    #[error("invalid bucket name")]
    InvalidBucketName,
    #[error("too many buckets")]
    TooManyBuckets,
    #[error("no such key")]
    NoSuchKey,
    #[error("invalid range")]
    InvalidRange,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("malformed xml")]
    MalformedXml,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("not implemented")]
    NotImplemented,
    #[error("internal error: {0}")]
    InternalError(String),
}

impl RgwError {
    /// The S3 `<Code>` element, also used as the admin API `"Code"` field.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AccessDenied => "AccessDenied",
            Self::InvalidAccessKeyId => "InvalidAccessKeyId",
            Self::SignatureDoesNotMatch => "SignatureDoesNotMatch",
            Self::RequestTimeTooSkewed => "RequestTimeTooSkewed",
            Self::AuthorizationHeaderMalformed(_) => "AuthorizationHeaderMalformed",
            Self::XAmzContentSha256Mismatch => "XAmzContentSHA256Mismatch",
            Self::BadDigest => "BadDigest",
            Self::NoSuchUser => "NoSuchUser",
            Self::UserAlreadyExists => "UserAlreadyExists",
            Self::KeyExists => "KeyExists",
            Self::NoSuchBucket => "NoSuchBucket",
            Self::BucketAlreadyExists => "BucketAlreadyExists",
            Self::BucketNotEmpty => "BucketNotEmpty",
            Self::InvalidBucketName => "InvalidBucketName",
            Self::TooManyBuckets => "TooManyBuckets",
            Self::NoSuchKey => "NoSuchKey",
            Self::InvalidRange => "InvalidRange",
            Self::InvalidArgument(_) => "InvalidArgument",
            Self::InvalidRequest(_) => "InvalidRequest",
            Self::MalformedXml => "MalformedXML",
            Self::MethodNotAllowed => "MethodNotAllowed",
            Self::NotImplemented => "NotImplemented",
            Self::InternalError(_) => "InternalError",
        }
    }

    /// The HTTP status RGW pairs with the code.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::AccessDenied
            | Self::InvalidAccessKeyId
            | Self::SignatureDoesNotMatch
            | Self::RequestTimeTooSkewed => 403,
            Self::AuthorizationHeaderMalformed(_)
            | Self::XAmzContentSha256Mismatch
            | Self::BadDigest
            | Self::InvalidBucketName
            | Self::TooManyBuckets
            | Self::InvalidArgument(_)
            | Self::InvalidRequest(_)
            | Self::MalformedXml => 400,
            Self::NoSuchUser | Self::NoSuchBucket | Self::NoSuchKey => 404,
            Self::MethodNotAllowed => 405,
            Self::UserAlreadyExists
            | Self::KeyExists
            | Self::BucketAlreadyExists
            | Self::BucketNotEmpty => 409,
            Self::InvalidRange => 416,
            Self::InternalError(_) => 500,
            Self::NotImplemented => 501,
        }
    }

    /// Wrap any error a driver or library surfaced.
    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::InternalError(err.to_string())
    }
}

pub type RgwResult<T> = Result<T, RgwError>;
