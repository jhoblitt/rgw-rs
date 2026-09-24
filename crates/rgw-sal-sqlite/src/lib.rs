//! A SAL driver on SQLite: the counterpart of `driver/dbstore`, which keeps
//! users, buckets and objects (data included) in one SQLite file.
//!
//! dbstore splits object data into per-part rows of an `ObjectData` table;
//! here each object's payload is one inline BLOB, so an object must fit in
//! memory.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use md5::{Digest, Md5};
use rgw_sal::{
    BucketList, ByteRange, DEFAULT_MAX_ENTRIES, Driver, ListParams, ListResult, ObjectBody,
    ObjectRead,
};
use rgw_types::{
    Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, RgwError, RgwResult,
    UserId, UserInfo,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users (
    uid TEXT PRIMARY KEY,
    email TEXT,
    info TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS users_email ON users(email);
CREATE TABLE IF NOT EXISTS access_keys (
    access_key TEXT PRIMARY KEY,
    uid TEXT NOT NULL REFERENCES users(uid) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS access_keys_uid ON access_keys(uid);
CREATE TABLE IF NOT EXISTS buckets (
    bucket_id TEXT PRIMARY KEY,
    tenant TEXT NOT NULL,
    name TEXT NOT NULL,
    owner TEXT NOT NULL,
    info TEXT NOT NULL,
    UNIQUE(tenant, name)
);
CREATE INDEX IF NOT EXISTS buckets_name ON buckets(name);
CREATE TABLE IF NOT EXISTS objects (
    bucket_id TEXT NOT NULL REFERENCES buckets(bucket_id),
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    etag TEXT NOT NULL,
    mtime TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_display TEXT NOT NULL,
    storage_class TEXT NOT NULL,
    attrs TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY(bucket_id, name)
);
";

/// The storage class every object gets; RGW's default placement names it so.
const STANDARD: &str = "STANDARD";

/// The columns [`object_info`] reads, in order.
const OBJECT_COLS: &str = "name, size, etag, mtime, owner_id, owner_display, storage_class, attrs";

/// `rgw::sal::DBStore` (`rgw_sal_dbstore.h`) over `rgw::store::SQLiteDB`
/// (`driver/dbstore/sqlite/sqliteDB.h`).
///
/// One connection behind a mutex: SQLite serializes writers anyway, and
/// every call runs on the blocking pool so the async runtime never waits on
/// disk.
pub struct SqliteDriver {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteDriver {
    /// Open or create the database at `path`, as `DBStoreManager` does for
    /// `dbstore_db_dir`/`dbstore_db_name_prefix`.
    pub fn open(path: &Path) -> RgwResult<Self> {
        let conn = Connection::open(path).map_err(RgwError::internal)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(RgwError::internal)?;
        Self::init(conn)
    }

    /// A private database that disappears on drop; for tests.
    pub fn open_in_memory() -> RgwResult<Self> {
        Self::init(Connection::open_in_memory().map_err(RgwError::internal)?)
    }

    fn init(conn: Connection) -> RgwResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON").map_err(RgwError::internal)?;
        conn.execute_batch(SCHEMA).map_err(RgwError::internal)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    async fn with_conn<T, F>(&self, f: F) -> RgwResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> RgwResult<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(RgwError::internal)?;
            f(&mut guard)
        })
        .await
        .map_err(RgwError::internal)?
    }

    async fn with_tx<T, F>(&self, f: F) -> RgwResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> RgwResult<T> + Send + 'static,
    {
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(RgwError::internal)?;
            let out = f(&tx)?;
            tx.commit().map_err(RgwError::internal)?;
            Ok(out)
        })
        .await
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> RgwResult<String> {
    serde_json::to_string(v).map_err(RgwError::internal)
}

fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> RgwResult<T> {
    serde_json::from_str(s).map_err(RgwError::internal)
}

fn fmt_time(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_time(s: &str) -> RgwResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).map(|t| t.with_timezone(&Utc)).map_err(RgwError::internal)
}

fn to_i64(n: u64) -> RgwResult<i64> {
    i64::try_from(n).map_err(RgwError::internal)
}

fn to_u64(n: i64) -> RgwResult<u64> {
    u64::try_from(n).map_err(RgwError::internal)
}

fn page_size(max: usize) -> usize {
    if max == 0 { DEFAULT_MAX_ENTRIES } else { max }
}

/// The raw columns of an object row, read without interpretation so row
/// decoding errors surface as `InternalError` rather than a rusqlite type.
struct ObjectRow {
    name: String,
    size: i64,
    etag: String,
    mtime: String,
    owner_id: String,
    owner_display: String,
    storage_class: String,
    attrs: String,
}

impl ObjectRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            name: row.get(0)?,
            size: row.get(1)?,
            etag: row.get(2)?,
            mtime: row.get(3)?,
            owner_id: row.get(4)?,
            owner_display: row.get(5)?,
            storage_class: row.get(6)?,
            attrs: row.get(7)?,
        })
    }

    fn into_info(self) -> RgwResult<ObjectInfo> {
        Ok(ObjectInfo {
            key: ObjectKey::new(self.name),
            size: to_u64(self.size)?,
            etag: self.etag,
            mtime: parse_time(&self.mtime)?,
            owner: Owner { id: UserId::parse(&self.owner_id), display_name: self.owner_display },
            storage_class: self.storage_class,
            attrs: from_json(&self.attrs)?,
        })
    }
}

fn check_bucket(conn: &Connection, bucket_id: &str) -> RgwResult<()> {
    conn.query_row("SELECT 1 FROM buckets WHERE bucket_id = ?1", [bucket_id], |_| Ok(()))
        .optional()
        .map_err(RgwError::internal)?
        .ok_or(RgwError::NoSuchBucket)
}

fn load_object(conn: &Connection, bucket_id: &str, name: &str) -> RgwResult<ObjectInfo> {
    check_bucket(conn, bucket_id)?;
    conn.query_row(
        &format!("SELECT {OBJECT_COLS} FROM objects WHERE bucket_id = ?1 AND name = ?2"),
        [bucket_id, name],
        ObjectRow::from_row,
    )
    .optional()
    .map_err(RgwError::internal)?
    .ok_or(RgwError::NoSuchKey)?
    .into_info()
}

fn user_by(conn: &Connection, sql: &str, arg: &str) -> RgwResult<UserInfo> {
    let info: String = conn
        .query_row(sql, [arg], |r| r.get(0))
        .optional()
        .map_err(RgwError::internal)?
        .ok_or(RgwError::NoSuchUser)?;
    from_json(&info)
}

/// Resolve a requested range against an object of `size` bytes into an
/// inclusive `(start, end)`, following the contract on
/// [`Driver::get_object`] (and `RGWGetObj::parse_range` in `rgw_op.cc`).
fn resolve_range(range: ByteRange, size: u64) -> RgwResult<(u64, u64)> {
    if size == 0 {
        return Err(RgwError::InvalidRange);
    }
    let last = size - 1;
    match range {
        ByteRange::Absolute { start, end } => {
            let end = end.map_or(last, |e| e.min(last));
            if start > last || end < start {
                return Err(RgwError::InvalidRange);
            }
            Ok((start, end))
        }
        ByteRange::Suffix(0) => Err(RgwError::InvalidRange),
        ByteRange::Suffix(n) => Ok((size - n.min(size), last)),
    }
}

/// `Bucket::list` over rows already filtered by prefix and marker and in
/// name order: the delimiter roll-up and paging that `RGWRados::Bucket::List`
/// does over the bucket index.
fn list_page(
    rows: impl Iterator<Item = RgwResult<ObjectRow>>,
    params: &ListParams,
) -> RgwResult<ListResult> {
    let max = page_size(params.max_keys);
    let mut out = ListResult::default();
    let mut last_prefix: Option<String> = None;
    for row in rows {
        let row = row?;
        // SQL filters by prefix already; this only guards the slicing.
        let Some(rest) = row.name.strip_prefix(params.prefix.as_str()) else {
            continue;
        };
        let common = if params.delimiter.is_empty() {
            None
        } else {
            rest.find(params.delimiter.as_str()).map(|i| {
                let end = params.prefix.len() + i + params.delimiter.len();
                row.name[..end].to_owned()
            })
        };
        if let Some(cp) = &common {
            // Keys sharing a prefix are contiguous in name order, so comparing
            // against the last one emitted is enough to deduplicate.
            if last_prefix.as_deref() == Some(cp.as_str()) || cp.as_str() <= params.marker.as_str() {
                continue;
            }
        }
        if out.objects.len() + out.common_prefixes.len() >= max {
            out.is_truncated = true;
            break;
        }
        match common {
            Some(cp) => {
                out.next_marker.clone_from(&cp);
                out.common_prefixes.push(cp.clone());
                last_prefix = Some(cp);
            }
            None => {
                out.next_marker.clone_from(&row.name);
                out.objects.push(row.into_info()?);
            }
        }
    }
    Ok(out)
}

#[async_trait]
impl Driver for SqliteDriver {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    /// `SQLGetUser` by uid.
    async fn load_user(&self, id: &UserId) -> RgwResult<UserInfo> {
        let uid = id.to_string();
        self.with_conn(move |c| user_by(c, "SELECT info FROM users WHERE uid = ?1", &uid)).await
    }

    /// `SQLGetUser` by access key.
    async fn load_user_by_access_key(&self, access_key: &str) -> RgwResult<UserInfo> {
        let key = access_key.to_owned();
        self.with_conn(move |c| {
            user_by(
                c,
                "SELECT u.info FROM access_keys k JOIN users u ON u.uid = k.uid WHERE k.access_key = ?1",
                &key,
            )
        })
        .await
    }

    /// `SQLGetUser` by email.
    async fn load_user_by_email(&self, email: &str) -> RgwResult<UserInfo> {
        if email.is_empty() {
            return Err(RgwError::NoSuchUser);
        }
        let email = email.to_owned();
        self.with_conn(move |c| {
            user_by(c, "SELECT info FROM users WHERE email = ?1 ORDER BY uid LIMIT 1", &email)
        })
        .await
    }

    /// `SQLInsertUser`, with the access key index rewritten in the same
    /// transaction.
    async fn store_user(&self, info: &UserInfo, exclusive: bool) -> RgwResult<()> {
        let uid = info.user_id.to_string();
        let email = (!info.user_email.is_empty()).then(|| info.user_email.clone());
        let json = to_json(info)?;
        let keys: Vec<String> = info.access_keys.keys().cloned().collect();
        self.with_tx(move |tx| {
            if exclusive {
                let exists = tx
                    .query_row("SELECT 1 FROM users WHERE uid = ?1", [&uid], |_| Ok(()))
                    .optional()
                    .map_err(RgwError::internal)?;
                if exists.is_some() {
                    return Err(RgwError::UserAlreadyExists);
                }
            }
            {
                let mut owner_of = tx
                    .prepare("SELECT uid FROM access_keys WHERE access_key = ?1")
                    .map_err(RgwError::internal)?;
                for key in &keys {
                    let owner: Option<String> =
                        owner_of.query_row([key], |r| r.get(0)).optional().map_err(RgwError::internal)?;
                    if owner.is_some_and(|o| o != uid) {
                        return Err(RgwError::KeyExists);
                    }
                }
            }
            tx.execute(
                "INSERT INTO users (uid, email, info) VALUES (?1, ?2, ?3)
                 ON CONFLICT(uid) DO UPDATE SET email = excluded.email, info = excluded.info",
                params![uid, email, json],
            )
            .map_err(RgwError::internal)?;
            tx.execute("DELETE FROM access_keys WHERE uid = ?1", [&uid]).map_err(RgwError::internal)?;
            let mut insert = tx
                .prepare("INSERT INTO access_keys (access_key, uid) VALUES (?1, ?2)")
                .map_err(RgwError::internal)?;
            for key in &keys {
                insert.execute([key, &uid]).map_err(RgwError::internal)?;
            }
            Ok(())
        })
        .await
    }

    /// `SQLRemoveUser`; the access key rows go with it by cascade.
    async fn remove_user(&self, id: &UserId) -> RgwResult<()> {
        let uid = id.to_string();
        self.with_conn(move |c| {
            match c.execute("DELETE FROM users WHERE uid = ?1", [&uid]).map_err(RgwError::internal)? {
                0 => Err(RgwError::NoSuchUser),
                _ => Ok(()),
            }
        })
        .await
    }

    /// `SQLListUsers`.
    async fn list_users(&self, marker: &str, max: usize) -> RgwResult<Vec<UserId>> {
        let marker = marker.to_owned();
        let limit = to_i64(page_size(max) as u64)?;
        self.with_conn(move |c| {
            let mut stmt = c
                .prepare("SELECT uid FROM users WHERE uid > ?1 ORDER BY uid LIMIT ?2")
                .map_err(RgwError::internal)?;
            let rows = stmt
                .query_map(params![marker, limit], |r| r.get::<_, String>(0))
                .map_err(RgwError::internal)?;
            rows.map(|r| r.map(|s| UserId::parse(&s)).map_err(RgwError::internal)).collect()
        })
        .await
    }

    /// `SQLInsertBucket`.
    async fn create_bucket(&self, info: &BucketInfo) -> RgwResult<()> {
        let b = &info.bucket;
        let (id, tenant, name) = (b.bucket_id.clone(), b.tenant.clone(), b.name.clone());
        let owner = info.owner.to_string();
        let json = to_json(info)?;
        self.with_tx(move |tx| {
            let exists = tx
                .query_row(
                    "SELECT 1 FROM buckets WHERE bucket_id = ?1 OR (tenant = ?2 AND name = ?3)",
                    [&id, &tenant, &name],
                    |_| Ok(()),
                )
                .optional()
                .map_err(RgwError::internal)?;
            if exists.is_some() {
                return Err(RgwError::BucketAlreadyExists);
            }
            tx.execute(
                "INSERT INTO buckets (bucket_id, tenant, name, owner, info) VALUES (?1, ?2, ?3, ?4, ?5)",
                [&id, &tenant, &name, &owner, &json],
            )
            .map_err(RgwError::internal)?;
            Ok(())
        })
        .await
    }

    /// `SQLGetBucket`.
    async fn load_bucket(&self, tenant: &str, name: &str) -> RgwResult<BucketInfo> {
        let (tenant, name) = (tenant.to_owned(), name.to_owned());
        self.with_conn(move |c| {
            let info: String = c
                .query_row(
                    "SELECT info FROM buckets WHERE tenant = ?1 AND name = ?2",
                    [&tenant, &name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(RgwError::internal)?
                .ok_or(RgwError::NoSuchBucket)?;
            from_json(&info)
        })
        .await
    }

    /// `SQLListUserBuckets`, or `DB::ListAllBuckets` when `owner` is `None`.
    async fn list_buckets(
        &self,
        owner: Option<&UserId>,
        marker: &str,
        max: usize,
    ) -> RgwResult<BucketList> {
        let owner = owner.map(UserId::to_string);
        let marker = marker.to_owned();
        let max = page_size(max);
        let limit = to_i64(max as u64 + 1)?;
        self.with_conn(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT info FROM buckets WHERE name > ?1 AND (?2 IS NULL OR owner = ?2)
                     ORDER BY name, tenant LIMIT ?3",
                )
                .map_err(RgwError::internal)?;
            let rows = stmt
                .query_map(params![marker, owner, limit], |r| r.get::<_, String>(0))
                .map_err(RgwError::internal)?;
            let mut out = BucketList::default();
            for row in rows {
                let info: BucketInfo = from_json(&row.map_err(RgwError::internal)?)?;
                if out.buckets.len() == max {
                    out.is_truncated = true;
                    break;
                }
                out.next_marker.clone_from(&info.bucket.name);
                out.buckets.push(info);
            }
            Ok(out)
        })
        .await
    }

    /// `SQLRemoveBucket`.
    async fn remove_bucket(&self, tenant: &str, name: &str) -> RgwResult<()> {
        let (tenant, name) = (tenant.to_owned(), name.to_owned());
        self.with_tx(move |tx| {
            let id: String = tx
                .query_row(
                    "SELECT bucket_id FROM buckets WHERE tenant = ?1 AND name = ?2",
                    [&tenant, &name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(RgwError::internal)?
                .ok_or(RgwError::NoSuchBucket)?;
            let has_objects = tx
                .query_row("SELECT 1 FROM objects WHERE bucket_id = ?1 LIMIT 1", [&id], |_| Ok(()))
                .optional()
                .map_err(RgwError::internal)?;
            if has_objects.is_some() {
                return Err(RgwError::BucketNotEmpty);
            }
            tx.execute("DELETE FROM buckets WHERE bucket_id = ?1", [&id]).map_err(RgwError::internal)?;
            Ok(())
        })
        .await
    }

    /// `Bucket::read_stats`, summed over the object rows; dbstore keeps no
    /// separate counters either.
    async fn bucket_stats(&self, bucket: &BucketKey) -> RgwResult<BucketStats> {
        let id = bucket.bucket_id.clone();
        self.with_conn(move |c| {
            check_bucket(c, &id)?;
            let (count, size): (i64, i64) = c
                .query_row(
                    "SELECT count(*), coalesce(sum(size), 0) FROM objects WHERE bucket_id = ?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(RgwError::internal)?;
            Ok(BucketStats { num_objects: to_u64(count)?, size: to_u64(size)? })
        })
        .await
    }

    /// `SQLListBucketObjects`.
    ///
    /// Keys that roll up into a common prefix are still read and skipped one
    /// by one, so a page after a large common prefix costs a scan of it.
    async fn list_objects(&self, bucket: &BucketKey, params: &ListParams) -> RgwResult<ListResult> {
        let id = bucket.bucket_id.clone();
        let params = params.clone();
        self.with_conn(move |c| {
            check_bucket(c, &id)?;
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {OBJECT_COLS} FROM objects
                     WHERE bucket_id = ?1 AND name > ?2 AND name >= ?3
                       AND substr(name, 1, length(?3)) = ?3
                     ORDER BY name"
                ))
                .map_err(RgwError::internal)?;
            let rows = stmt
                .query_map(params![id, params.marker, params.prefix], ObjectRow::from_row)
                .map_err(RgwError::internal)?;
            list_page(rows.map(|r| r.map_err(RgwError::internal)), &params)
        })
        .await
    }

    /// `SQLGetObject`.
    async fn head_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<ObjectInfo> {
        let (id, name) = (bucket.bucket_id.clone(), key.name.clone());
        self.with_conn(move |c| load_object(c, &id, &name)).await
    }

    /// `SQLGetObject` + `SQLGetObjectData`; a range reads only its bytes.
    async fn get_object(
        &self,
        bucket: &BucketKey,
        key: &ObjectKey,
        range: Option<ByteRange>,
    ) -> RgwResult<ObjectRead> {
        let (id, name) = (bucket.bucket_id.clone(), key.name.clone());
        let (info, range, data) = self
            .with_conn(move |c| {
                let info = load_object(c, &id, &name)?;
                let range = range.map(|r| resolve_range(r, info.size)).transpose()?;
                // SQLite's substr on a BLOB is 1-based and counts bytes.
                let (from, len) = match range {
                    Some((start, end)) => (to_i64(start + 1)?, to_i64(end - start + 1)?),
                    None => (1, to_i64(info.size)?),
                };
                // substr() of a zero-length BLOB is NULL, not an empty BLOB,
                // and an empty object admits no range, so there is nothing to
                // read.
                if len == 0 {
                    return Ok((info, range, Vec::new()));
                }
                let data: Vec<u8> = c
                    .query_row(
                        "SELECT substr(data, ?3, ?4) FROM objects WHERE bucket_id = ?1 AND name = ?2",
                        params![id, name, from, len],
                        |r| r.get(0),
                    )
                    .map_err(RgwError::internal)?;
                Ok((info, range, data))
            })
            .await?;
        Ok(ObjectRead { info, range, body: rgw_sal::body_from_bytes(data) })
    }

    /// `SQLPutObject` + `SQLPutObjectData`, as the atomic writer's
    /// `complete` does them.
    ///
    /// The object's version instance is ignored: there is no versioning, so
    /// a stored object always reads back with an empty instance.
    async fn put_object(
        &self,
        bucket: &BucketKey,
        key: &ObjectKey,
        owner: Owner,
        attrs: Attrs,
        body: ObjectBody,
    ) -> RgwResult<ObjectInfo> {
        let data = rgw_sal::read_body(body).await.map_err(RgwError::internal)?;
        let info = ObjectInfo {
            key: ObjectKey::new(key.name.clone()),
            size: data.len() as u64,
            etag: hex::encode(Md5::digest(&data)),
            mtime: Utc::now(),
            owner,
            storage_class: STANDARD.to_owned(),
            attrs,
        };
        let id = bucket.bucket_id.clone();
        let row = (
            info.key.name.clone(),
            to_i64(info.size)?,
            info.etag.clone(),
            fmt_time(&info.mtime),
            info.owner.id.to_string(),
            info.owner.display_name.clone(),
            info.storage_class.clone(),
            to_json(&info.attrs)?,
        );
        self.with_tx(move |tx| {
            check_bucket(tx, &id)?;
            let (name, size, etag, mtime, owner_id, owner_display, storage_class, attrs) = row;
            tx.execute(
                "INSERT OR REPLACE INTO objects
                 (bucket_id, name, size, etag, mtime, owner_id, owner_display, storage_class, attrs, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![id, name, size, etag, mtime, owner_id, owner_display, storage_class, attrs, &data[..]],
            )
            .map_err(RgwError::internal)?;
            Ok(())
        })
        .await?;
        Ok(info)
    }

    /// `SQLDeleteObject` + `SQLDeleteObjectData`.
    async fn delete_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<()> {
        let (id, name) = (bucket.bucket_id.clone(), key.name.clone());
        self.with_tx(move |tx| {
            check_bucket(tx, &id)?;
            match tx
                .execute("DELETE FROM objects WHERE bucket_id = ?1 AND name = ?2", [&id, &name])
                .map_err(RgwError::internal)?
            {
                0 => Err(RgwError::NoSuchKey),
                _ => Ok(()),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;
    use rgw_types::AccessKey;

    use super::*;

    #[tokio::test]
    async fn conformance() {
        let driver = SqliteDriver::open_in_memory().unwrap();
        rgw_sal::testsuite::run_all(&driver).await;
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rgw.db");
        let now = Utc::now().with_nanosecond(0).unwrap();

        let mut user = UserInfo::new(UserId::with_tenant("acme", "dora"), "Dora", now);
        user.add_access_key(AccessKey {
            id: "AKDORA".to_owned(),
            key: "secret".to_owned(),
            subuser: String::new(),
            active: true,
            create_date: now,
        });
        let bucket = BucketInfo {
            bucket: BucketKey::new("acme", "persist", "id-persist"),
            owner: user.user_id.clone(),
            creation_time: now,
            placement_rule: "default-placement".to_owned(),
            zonegroup: "default".to_owned(),
            num_shards: 0,
        };
        let key = ObjectKey::new("k");
        let owner = Owner { id: user.user_id.clone(), display_name: "Dora".to_owned() };

        let put = {
            let d = SqliteDriver::open(&path).unwrap();
            d.store_user(&user, true).await.unwrap();
            d.create_bucket(&bucket).await.unwrap();
            d.put_object(&bucket.bucket, &key, owner, Attrs::new(), rgw_sal::body_from_bytes("persisted"))
                .await
                .unwrap()
        };

        let d = SqliteDriver::open(&path).unwrap();
        assert_eq!(d.load_user(&user.user_id).await.unwrap(), user);
        assert_eq!(d.load_user_by_access_key("AKDORA").await.unwrap(), user);
        assert_eq!(d.load_bucket("acme", "persist").await.unwrap(), bucket);
        let read = d.get_object(&bucket.bucket, &key, None).await.unwrap();
        assert_eq!(read.info, put);
        assert_eq!(rgw_sal::read_body(read.body).await.unwrap(), "persisted");
    }

    #[test]
    fn range_rejects_inverted_bounds() {
        let r = resolve_range(ByteRange::Absolute { start: 5, end: Some(3) }, 10);
        assert!(matches!(r, Err(RgwError::InvalidRange)));
    }
}
