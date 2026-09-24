//! Admin operations shared by the REST admin API and `radosgw-admin`: the
//! counterpart of `RGWUserAdminOp_*` in `rgw_user.cc` and `RGWBucketAdminOp`
//! in `rgw_bucket.cc`. Everything here goes through the SAL trait, which is
//! what lets both front ends stay backend-neutral; in RGW today the REST
//! half is registered by the RADOS driver.
//!
//! The JSON shapes (`RGWUserInfo::dump`, `RGWBucketAdminOp::info`) live in
//! [`dump`] so both front ends print the same document.

/// STUB: the admin worker fills this crate in.
pub mod dump {}
