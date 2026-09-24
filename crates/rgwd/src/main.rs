use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use rgw_sal_sqlite::SqliteDriver;

/// The rgw-rs gateway daemon.
#[derive(Parser, Debug)]
#[command(name = "rgwd", version)]
struct Args {
    /// Address to listen on (`rgw_frontends` port; RGW's default is 7480).
    #[arg(long, default_value = "127.0.0.1:7480", env = "RGWD_LISTEN")]
    listen: SocketAddr,

    /// SQLite database file (`dbstore_db_dir` in RGW).
    #[arg(long, default_value = "rgw.db", env = "RGWD_DB")]
    db: PathBuf,

    /// URL prefix for the admin API (`rgw_admin_entry`).
    #[arg(long, default_value = rgwd::DEFAULT_ADMIN_ENTRY, env = "RGWD_ADMIN_ENTRY")]
    admin_entry: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    let driver = Arc::new(SqliteDriver::open(&args.db)?);
    let app = rgwd::build_app(driver, &args.admin_entry);
    rgwd::serve(app, args.listen).await
}
