//! The admin CLI: the counterpart of `radosgw-admin.cc`. Like the C++ tool
//! it links the driver and talks to storage directly rather than through
//! the REST API, sharing its operations with `rgw-rest-admin` through
//! `rgw-admin-ops`, so both print the same documents.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Parser, Subcommand};
use rgw_admin_ops::{self as ops, UserCreateParams, UserModifyParams, dump};
use rgw_sal::Driver;
use rgw_sal_sqlite::SqliteDriver;
use rgw_types::{RgwError, RgwResult, UserId};
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(name = "radosgw-admin", version)]
struct Args {
    /// SQLite database file shared with rgwd.
    #[arg(long, default_value = "rgw.db", env = "RGWD_DB")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// User operations.
    User {
        #[command(subcommand)]
        op: UserOp,
    },
    /// S3 access key operations.
    Key {
        #[command(subcommand)]
        op: KeyOp,
    },
    /// Admin capability operations.
    Caps {
        #[command(subcommand)]
        op: CapsOp,
    },
    /// Bucket operations.
    Bucket {
        #[command(subcommand)]
        op: BucketOp,
    },
}

#[derive(Subcommand, Debug)]
enum UserOp {
    /// Create a new user.
    Create(UserFlags),
    /// Print a user's info.
    Info(UserFlags),
    /// Modify a user.
    Modify(UserFlags),
    /// Remove a user.
    Rm(UserFlags),
    /// List user ids.
    List {
        #[arg(long)]
        marker: Option<String>,
        /// Print one page as `{keys, truncated, count[, marker]}` instead of
        /// every id.
        #[arg(long)]
        max_entries: Option<usize>,
    },
}

#[derive(Subcommand, Debug)]
enum KeyOp {
    /// Add an access key to a user, or replace a key's secret.
    Create(UserFlags),
    /// Remove an access key.
    Rm(UserFlags),
}

#[derive(Subcommand, Debug)]
enum CapsOp {
    /// Add caps, e.g. `--caps "users=*;buckets=read"`.
    Add(UserFlags),
    /// Remove caps.
    Rm(UserFlags),
}

#[derive(Subcommand, Debug)]
enum BucketOp {
    /// List bucket names, all or one user's.
    List {
        #[arg(long)]
        uid: Option<String>,
    },
    /// Print a bucket's info and usage.
    Stats {
        /// `name` or `tenant/name`.
        #[arg(long)]
        bucket: String,
    },
    /// Remove a bucket.
    Rm {
        #[arg(long)]
        bucket: String,
        #[arg(long)]
        purge_objects: bool,
    },
}

/// The user flags `radosgw-admin.cc` parses globally; each command reads
/// the ones it needs and the op reports what is missing.
#[derive(ClapArgs, Debug, Default)]
struct UserFlags {
    #[arg(long)]
    uid: Option<String>,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    email: Option<String>,
    #[arg(long)]
    access_key: Option<String>,
    #[arg(long)]
    secret_key: Option<String>,
    #[arg(long)]
    gen_access_key: bool,
    #[arg(long)]
    gen_secret: bool,
    #[arg(long)]
    caps: Option<String>,
    #[arg(long)]
    max_buckets: Option<i32>,
    /// `--suspended` alone means true.
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_parser = parse_bool)]
    suspended: Option<bool>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_parser = parse_bool)]
    system: Option<bool>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_parser = parse_bool)]
    admin: Option<bool>,
    #[arg(long)]
    purge_data: bool,
}

impl UserFlags {
    fn uid(&self) -> RgwResult<UserId> {
        match self.uid.as_deref() {
            Some(uid) if !uid.is_empty() => Ok(UserId::parse(uid)),
            _ => Err(RgwError::InvalidArgument("--uid is required".into())),
        }
    }

    fn generate(&self) -> bool {
        self.gen_access_key || self.gen_secret
    }
}

/// The same vocabulary the REST API accepts (`RESTArgs::get_bool`).
fn parse_bool(s: &str) -> Result<bool, String> {
    match s {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        other => Err(format!("not a boolean: {other}")),
    }
}

/// What a command prints: a formatter document, or nothing.
type Output = Option<Value>;

async fn run(driver: &dyn Driver, command: Command) -> RgwResult<Output> {
    match command {
        Command::User { op } => user(driver, op).await,
        Command::Key { op } => key(driver, op).await,
        Command::Caps { op } => caps(driver, op).await,
        Command::Bucket { op } => bucket(driver, op).await,
    }
}

async fn user(driver: &dyn Driver, op: UserOp) -> RgwResult<Output> {
    let info = match op {
        UserOp::Create(f) => {
            let params = UserCreateParams {
                uid: f.uid()?,
                display_name: f.display_name.clone().unwrap_or_default(),
                email: f.email.clone(),
                access_key: f.access_key.clone(),
                secret_key: f.secret_key.clone(),
                generate_key: f.generate().then_some(true),
                caps: f.caps.clone(),
                max_buckets: f.max_buckets,
                suspended: f.suspended,
                system: f.system,
                admin: f.admin,
            };
            ops::user_create(driver, params).await?
        }
        UserOp::Info(f) => ops::user_info(driver, &f.uid()?).await?,
        UserOp::Modify(f) => {
            let params = UserModifyParams {
                uid: f.uid()?,
                display_name: f.display_name.clone(),
                email: f.email.clone(),
                max_buckets: f.max_buckets,
                suspended: f.suspended,
                system: f.system,
                admin: f.admin,
                access_key: f.access_key.clone(),
                secret_key: f.secret_key.clone(),
                generate_key: f.generate(),
            };
            ops::user_modify(driver, params).await?
        }
        UserOp::Rm(f) => {
            ops::user_remove(driver, &f.uid()?, f.purge_data).await?;
            return Ok(None);
        }
        UserOp::List { marker, max_entries } => {
            let marker = marker.unwrap_or_default();
            if let Some(max) = max_entries {
                let page = ops::user_list(driver, &marker, max).await?;
                return Ok(Some(dump::user_list(&page.ids, page.truncated, &page.next_marker)));
            }
            return list_all_users(driver, marker).await.map(Some);
        }
    };
    Ok(Some(dump::user_info(&info)))
}

/// `radosgw-admin user list` without `--max-entries` pages through every
/// id and prints a bare array, as the C++ tool's metadata listing does.
async fn list_all_users(driver: &dyn Driver, mut marker: String) -> RgwResult<Value> {
    let mut ids = Vec::new();
    loop {
        let page = ops::user_list(driver, &marker, 0).await?;
        ids.extend(page.ids.iter().map(|id| Value::String(id.to_string())));
        if !page.truncated {
            return Ok(Value::Array(ids));
        }
        marker = page.next_marker;
    }
}

async fn key(driver: &dyn Driver, op: KeyOp) -> RgwResult<Output> {
    let info = match op {
        KeyOp::Create(f) => {
            let generate = f.generate() || f.access_key.is_none();
            ops::key_create(driver, &f.uid()?, f.access_key.as_deref(), f.secret_key.as_deref(), generate)
                .await?
        }
        KeyOp::Rm(f) => {
            let access_key = f.access_key.as_deref().unwrap_or_default();
            ops::key_remove(driver, &f.uid()?, access_key).await?
        }
    };
    Ok(Some(dump::user_info(&info)))
}

async fn caps(driver: &dyn Driver, op: CapsOp) -> RgwResult<Output> {
    let info = match op {
        CapsOp::Add(f) => ops::caps_add(driver, &f.uid()?, f.caps.as_deref().unwrap_or_default()).await?,
        CapsOp::Rm(f) => ops::caps_remove(driver, &f.uid()?, f.caps.as_deref().unwrap_or_default()).await?,
    };
    Ok(Some(dump::user_info(&info)))
}

async fn bucket(driver: &dyn Driver, op: BucketOp) -> RgwResult<Output> {
    match op {
        BucketOp::List { uid } => {
            let owner = uid.as_deref().filter(|u| !u.is_empty()).map(UserId::parse);
            let buckets = ops::bucket_list(driver, owner.as_ref()).await?;
            Ok(Some(buckets.into_iter().map(|b| Value::String(b.bucket.name)).collect()))
        }
        BucketOp::Stats { bucket } => {
            let (tenant, name) = ops::parse_bucket(&bucket);
            let (info, stats) = ops::bucket_info(driver, &tenant, &name).await?;
            Ok(Some(dump::bucket_info(&info, Some(&stats))))
        }
        BucketOp::Rm { bucket, purge_objects } => {
            let (tenant, name) = ops::parse_bucket(&bucket);
            ops::bucket_remove(driver, &tenant, &name, purge_objects).await?;
            Ok(None)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let result = match SqliteDriver::open(&args.db) {
        Ok(driver) => run(&driver, args.command).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(Some(doc)) => match serde_json::to_string_pretty(&doc) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("ERROR: InternalError: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {}: {e}", e.code());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("radosgw-admin").chain(argv.iter().copied())).unwrap()
    }

    #[test]
    fn cli_is_well_formed() {
        Args::command().debug_assert();
    }

    #[test]
    fn user_create_flags() {
        let args = parse(&[
            "--db", "x.db", "user", "create", "--uid", "acme$alice", "--display-name", "Alice",
            "--caps", "users=*", "--max-buckets", "5", "--system", "--admin", "false", "--gen-secret",
        ]);
        assert_eq!(args.db, PathBuf::from("x.db"));
        let Command::User { op: UserOp::Create(f) } = args.command else { panic!("{:?}", args.command) };
        assert_eq!(f.uid().unwrap(), UserId::with_tenant("acme", "alice"));
        assert_eq!(f.display_name.as_deref(), Some("Alice"));
        assert_eq!(f.max_buckets, Some(5));
        assert_eq!(f.system, Some(true));
        assert_eq!(f.admin, Some(false));
        assert_eq!(f.suspended, None);
        assert!(f.generate());
    }

    #[test]
    fn bucket_and_key_commands_parse() {
        let args = parse(&["bucket", "rm", "--bucket", "t/b1", "--purge-objects"]);
        assert!(matches!(args.command, Command::Bucket { op: BucketOp::Rm { purge_objects: true, .. } }));
        let args = parse(&["key", "rm", "--uid", "alice", "--access-key", "AK"]);
        assert!(matches!(args.command, Command::Key { op: KeyOp::Rm(_) }));
        let args = parse(&["caps", "add", "--uid", "alice", "--caps", "users=read"]);
        assert!(matches!(args.command, Command::Caps { op: CapsOp::Add(_) }));
        assert!(Args::try_parse_from(["radosgw-admin", "user", "create", "--suspended", "maybe"]).is_err());
    }

    #[test]
    fn missing_uid_is_invalid_argument() {
        assert!(matches!(UserFlags::default().uid(), Err(RgwError::InvalidArgument(_))));
    }
}
