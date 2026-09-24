//! Admin operations shared by the REST admin API and `radosgw-admin`: the
//! counterpart of `RGWUserAdminOp_*` in `rgw_user.cc` and `RGWBucketAdminOp`
//! in `rgw_bucket.cc`. Everything here goes through the SAL trait, which is
//! what lets both front ends stay backend-neutral; in RGW today the REST
//! half is registered by the RADOS driver.
//!
//! The JSON shapes (`RGWUserInfo::dump`, `RGWBucketAdminOp::info`) live in
//! [`dump`] so both front ends print the same document.

mod bucket;
pub mod dump;
mod keygen;
mod user;

pub use bucket::{bucket_info, bucket_list, bucket_remove, parse_bucket};
pub use keygen::{ACCESS_KEY_LEN, SECRET_KEY_LEN, generate_access_key, generate_secret_key};
pub use user::{
    UserCreateParams, UserList, UserModifyParams, caps_add, caps_remove, key_create, key_remove,
    user_create, user_info, user_list, user_modify, user_remove,
};
