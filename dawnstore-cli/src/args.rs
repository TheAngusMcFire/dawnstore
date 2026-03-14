use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kubectl-lite",
    version,
    about = "A simplified kubectl-like CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(short, long)]
    pub namespace: Option<String>,

    #[arg(short, long, env = "DAWNSTORE_CONTEXT")]
    pub context_path: String,

    #[arg(short = 'A', long)]
    pub all_namespaces: bool,

    /// Bearer token for API authentication.
    /// Takes precedence over the token in the context file.
    /// Can also be set via the DAWNSTORE_TOKEN environment variable.
    #[arg(long, env = "DAWNSTORE_TOKEN")]
    pub token: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Display one or many resources
    Get { resource: String },
    /// Delete resources
    Delete { resource: String, item_name: String },
    /// Edit resource
    Edit { resource: String, item_name: String },
    /// Apply resource from file
    Apply { path: String },
}
