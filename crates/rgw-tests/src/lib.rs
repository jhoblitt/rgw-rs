//! Test harness: hosts `rgwd`'s router in-process on an ephemeral port so
//! the integration tests under `tests/` can drive it with a real S3 SDK
//! and hand-signed admin requests.
//!
//! STUB: the integration-test worker fills in `TestServer`.
