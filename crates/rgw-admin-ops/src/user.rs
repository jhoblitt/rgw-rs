//! User, key and caps operations: `RGWUserAdminOp_User`, `RGWUserAdminOp_Key`
//! and `RGWUserAdminOp_Caps` in `rgw_user.cc`, with the checks `RGWUser`,
//! `RGWAccessKeyPool` and `RGWUserCapPool` perform folded in.

use chrono::Utc;
use rgw_sal::{DEFAULT_MAX_ENTRIES, Driver};
use rgw_types::{AccessKey, RgwError, RgwResult, UserCaps, UserId, UserInfo};

use crate::bucket::{list_all_buckets, purge_objects};
use crate::keygen::{generate_access_key, generate_secret_key};

/// Generated access keys are retried this often on a collision before
/// giving up, as `RGWAccessKeyPool::generate_key` loops on `-EEXIST`.
const KEYGEN_ATTEMPTS: usize = 10;

/// The inputs of `RGWUserAdminOp_User::create`, the subset of
/// `RGWUserAdminOpState` this spike supports.
#[derive(Clone, Debug, Default)]
pub struct UserCreateParams {
    pub uid: UserId,
    pub display_name: String,
    pub email: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    /// `None` resolves to true when neither key half is given, as RGW does.
    pub generate_key: Option<bool>,
    /// `users=*;buckets=read`, parsed by `UserCaps::add_from_string`.
    pub caps: Option<String>,
    pub max_buckets: Option<i32>,
    pub suspended: Option<bool>,
    pub system: Option<bool>,
    pub admin: Option<bool>,
}

/// The inputs of `RGWUserAdminOp_User::modify`; `None` leaves a field as is.
#[derive(Clone, Debug, Default)]
pub struct UserModifyParams {
    pub uid: UserId,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub max_buckets: Option<i32>,
    pub suspended: Option<bool>,
    pub system: Option<bool>,
    pub admin: Option<bool>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub generate_key: bool,
}

/// One page of `RGWUserAdminOp_User::list`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserList {
    pub ids: Vec<UserId>,
    pub truncated: bool,
    /// The marker for the next page; empty unless `truncated`.
    pub next_marker: String,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

fn require_uid(uid: &UserId) -> RgwResult<()> {
    if uid.is_empty() {
        return Err(RgwError::InvalidArgument("uid is required".into()));
    }
    Ok(())
}

/// `RGWUser::check_op`'s email clash test: `EmailExists` in RGW, which this
/// spike's error table does not carry, so `UserAlreadyExists` stands in.
async fn ensure_email_free(driver: &dyn Driver, uid: &UserId, email: &str) -> RgwResult<()> {
    match driver.load_user_by_email(email).await {
        Ok(other) if other.user_id != *uid => Err(RgwError::UserAlreadyExists),
        Ok(_) | Err(RgwError::NoSuchUser) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn ensure_key_free(driver: &dyn Driver, uid: &UserId, access_key: &str) -> RgwResult<()> {
    match driver.load_user_by_access_key(access_key).await {
        Ok(other) if other.user_id != *uid => Err(RgwError::KeyExists),
        Ok(_) | Err(RgwError::NoSuchUser) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn unused_access_key(driver: &dyn Driver) -> RgwResult<String> {
    for _ in 0..KEYGEN_ATTEMPTS {
        let candidate = generate_access_key();
        match driver.load_user_by_access_key(&candidate).await {
            Err(RgwError::NoSuchUser) => return Ok(candidate),
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(RgwError::internal("could not generate a unique access key"))
}

/// `RGWAccessKeyPool::add`: set or replace one S3 key on `info`. A missing
/// half is generated when `generate` is set and is an error otherwise.
async fn put_key(
    driver: &dyn Driver,
    info: &mut UserInfo,
    access_key: Option<&str>,
    secret_key: Option<&str>,
    generate: bool,
) -> RgwResult<()> {
    let id = match access_key {
        Some(id) => {
            ensure_key_free(driver, &info.user_id, id).await?;
            id.to_owned()
        }
        None if generate => unused_access_key(driver).await?,
        None => return Err(RgwError::InvalidArgument("access-key is required".into())),
    };
    let key = match secret_key {
        Some(key) => key.to_owned(),
        None if generate => generate_secret_key(),
        None => return Err(RgwError::InvalidArgument("secret-key is required".into())),
    };
    info.add_access_key(AccessKey { id, key, subuser: String::new(), active: true, create_date: Utc::now() });
    Ok(())
}

/// Resolve the key inputs of create/modify: `Some((access, secret,
/// generate))` when a key is to be added.
fn key_request<'a>(
    access_key: Option<&'a str>,
    secret_key: Option<&'a str>,
    generate: bool,
) -> RgwResult<Option<(Option<&'a str>, Option<&'a str>, bool)>> {
    match (access_key, secret_key, generate) {
        (None, None, false) => Ok(None),
        (Some(_), None, false) | (None, Some(_), false) => Err(RgwError::InvalidArgument(
            "access-key and secret-key must be given together".into(),
        )),
        (a, s, g) => Ok(Some((a, s, g))),
    }
}

/// `RGWUserAdminOp_User::create`.
///
/// The record is stored exclusively, so an existing uid is
/// `UserAlreadyExists`; an email already held by another user is too (RGW
/// says `EmailExists`).
pub async fn user_create(driver: &dyn Driver, params: UserCreateParams) -> RgwResult<UserInfo> {
    require_uid(&params.uid)?;
    if params.display_name.is_empty() {
        return Err(RgwError::InvalidArgument("display-name is required".into()));
    }
    let access_key = nonempty(&params.access_key);
    let secret_key = nonempty(&params.secret_key);
    let generate = params.generate_key.unwrap_or(access_key.is_none() && secret_key.is_none());
    let key = key_request(access_key, secret_key, generate)?;

    let mut info = UserInfo::new(params.uid.clone(), params.display_name.clone(), Utc::now());
    if let Some(caps) = nonempty(&params.caps) {
        info.caps.add_from_string(caps)?;
    }
    if let Some(email) = nonempty(&params.email) {
        ensure_email_free(driver, &info.user_id, email).await?;
        info.user_email = email.to_owned();
    }
    if let Some(max) = params.max_buckets {
        info.max_buckets = max;
    }
    info.suspended = params.suspended.unwrap_or(false);
    info.system = params.system.unwrap_or(false);
    info.admin = params.admin.unwrap_or(false);
    if let Some((a, s, g)) = key {
        put_key(driver, &mut info, a, s, g).await?;
    }
    driver.store_user(&info, true).await?;
    Ok(info)
}

/// `RGWUserAdminOp_User::info`.
pub async fn user_info(driver: &dyn Driver, uid: &UserId) -> RgwResult<UserInfo> {
    require_uid(uid)?;
    driver.load_user(uid).await
}

/// `RGWUserAdminOp_User::modify`. A key pair, or `generate_key`, adds a
/// key alongside the existing ones.
pub async fn user_modify(driver: &dyn Driver, params: UserModifyParams) -> RgwResult<UserInfo> {
    require_uid(&params.uid)?;
    let key = key_request(nonempty(&params.access_key), nonempty(&params.secret_key), params.generate_key)?;
    let mut info = driver.load_user(&params.uid).await?;
    if let Some(name) = &params.display_name {
        if name.is_empty() {
            return Err(RgwError::InvalidArgument("display-name must not be empty".into()));
        }
        info.display_name = name.clone();
    }
    if let Some(email) = &params.email {
        if !email.is_empty() {
            ensure_email_free(driver, &info.user_id, email).await?;
        }
        info.user_email = email.clone();
    }
    if let Some(max) = params.max_buckets {
        info.max_buckets = max;
    }
    if let Some(v) = params.suspended {
        info.suspended = v;
    }
    if let Some(v) = params.system {
        info.system = v;
    }
    if let Some(v) = params.admin {
        info.admin = v;
    }
    if let Some((a, s, g)) = key {
        put_key(driver, &mut info, a, s, g).await?;
    }
    driver.store_user(&info, false).await?;
    Ok(info)
}

/// `RGWUserAdminOp_User::remove`. A user who owns buckets is refused
/// unless `purge_data`, which deletes every object and bucket they own
/// first (`RGWUser::execute_remove`).
pub async fn user_remove(driver: &dyn Driver, uid: &UserId, purge_data: bool) -> RgwResult<()> {
    require_uid(uid)?;
    driver.load_user(uid).await?;
    if !purge_data {
        let first = driver.list_buckets(Some(uid), "", 1).await?;
        if !first.buckets.is_empty() {
            return Err(RgwError::InvalidRequest("user has buckets; use purge-data".into()));
        }
    } else {
        for bucket in list_all_buckets(driver, Some(uid)).await? {
            purge_objects(driver, &bucket.bucket).await?;
            driver.remove_bucket(&bucket.bucket.tenant, &bucket.bucket.name).await?;
        }
    }
    driver.remove_user(uid).await
}

/// `RGWUserAdminOp_User::list` (`RGWUser::list`): ids after `marker`, at
/// most `max`, clamped to 1000 as RGW does; 0 means 1000.
pub async fn user_list(driver: &dyn Driver, marker: &str, max: usize) -> RgwResult<UserList> {
    let max = if max == 0 { DEFAULT_MAX_ENTRIES } else { max.min(DEFAULT_MAX_ENTRIES) };
    let mut ids = driver.list_users(marker, max + 1).await?;
    let truncated = ids.len() > max;
    ids.truncate(max);
    let next_marker = match ids.last() {
        Some(last) if truncated => last.to_string(),
        _ => String::new(),
    };
    Ok(UserList { ids, truncated, next_marker })
}

/// `RGWUserAdminOp_Key::create`: add a key, or replace the secret of one
/// the user already holds.
pub async fn key_create(
    driver: &dyn Driver,
    uid: &UserId,
    access_key: Option<&str>,
    secret_key: Option<&str>,
    generate: bool,
) -> RgwResult<UserInfo> {
    require_uid(uid)?;
    let mut info = driver.load_user(uid).await?;
    let access_key = access_key.filter(|s| !s.is_empty());
    let secret_key = secret_key.filter(|s| !s.is_empty());
    put_key(driver, &mut info, access_key, secret_key, generate).await?;
    driver.store_user(&info, false).await?;
    Ok(info)
}

/// `RGWUserAdminOp_Key::remove`. A key the user does not hold is
/// `InvalidAccessKeyId` (`ERR_INVALID_ACCESS_KEY`).
pub async fn key_remove(driver: &dyn Driver, uid: &UserId, access_key: &str) -> RgwResult<UserInfo> {
    require_uid(uid)?;
    if access_key.is_empty() {
        return Err(RgwError::InvalidArgument("access-key is required".into()));
    }
    let mut info = driver.load_user(uid).await?;
    if info.access_keys.remove(access_key).is_none() {
        return Err(RgwError::InvalidAccessKeyId);
    }
    driver.store_user(&info, false).await?;
    Ok(info)
}

fn parse_caps(caps: &str) -> RgwResult<UserCaps> {
    if caps.trim().is_empty() {
        return Err(RgwError::InvalidArgument("user-caps is required".into()));
    }
    let mut parsed = UserCaps::new();
    parsed.add_from_string(caps)?;
    Ok(parsed)
}

/// `RGWUserAdminOp_Caps::add`.
pub async fn caps_add(driver: &dyn Driver, uid: &UserId, caps: &str) -> RgwResult<UserInfo> {
    require_uid(uid)?;
    let parsed = parse_caps(caps)?;
    let mut info = driver.load_user(uid).await?;
    for (cap, perm) in parsed.iter() {
        info.caps.add(cap, perm)?;
    }
    driver.store_user(&info, false).await?;
    Ok(info)
}

/// `RGWUserAdminOp_Caps::remove`.
pub async fn caps_remove(driver: &dyn Driver, uid: &UserId, caps: &str) -> RgwResult<UserInfo> {
    require_uid(uid)?;
    let parsed = parse_caps(caps)?;
    let mut info = driver.load_user(uid).await?;
    for (cap, perm) in parsed.iter() {
        info.caps.remove(cap, perm);
    }
    driver.store_user(&info, false).await?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_request_needs_both_halves_without_generate() {
        assert_eq!(key_request(None, None, false).unwrap(), None);
        assert!(key_request(Some("AK"), None, false).is_err());
        assert!(key_request(None, Some("SK"), false).is_err());
        assert_eq!(key_request(Some("AK"), Some("SK"), false).unwrap(), Some((Some("AK"), Some("SK"), false)));
        assert_eq!(key_request(Some("AK"), None, true).unwrap(), Some((Some("AK"), None, true)));
        assert_eq!(key_request(None, None, true).unwrap(), Some((None, None, true)));
    }

    #[test]
    fn caps_string_must_be_nonempty_and_valid() {
        assert!(matches!(parse_caps(""), Err(RgwError::InvalidArgument(_))));
        assert!(matches!(parse_caps("bogus=*"), Err(RgwError::InvalidArgument(_))));
        let caps = parse_caps("users=*;buckets=read").unwrap();
        assert_eq!(caps.iter().count(), 2);
    }
}
