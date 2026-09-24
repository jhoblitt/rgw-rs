//! The admin CLI: the counterpart of `radosgw-admin.cc`. Like the C++ tool
//! it links the driver and talks to storage directly rather than through
//! the REST API, sharing its operations with `rgw-rest-admin` through
//! `rgw-admin-ops`.
//!
//! STUB: the admin worker fills in the subcommands.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
}

#[derive(Subcommand, Debug)]
enum UserOp {
    /// List user ids.
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::bail!("not implemented: {:?}", args.command)
}
