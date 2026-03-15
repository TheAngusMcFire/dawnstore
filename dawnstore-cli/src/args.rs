use clap::{CommandFactory, Parser, Subcommand};

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
pub enum CreationKind {
    /// Issue a JWT for a service account
    Token {
        token_name: String,
        service_account: String,
    },
    /// Create a namespace (always stored in the system namespace)
    #[command(name = "namespace", alias = "ns")]
    Namespace {
        /// Name of the namespace to create
        name: String,
    },
    /// Create a service account in the current namespace
    #[command(name = "serviceaccount", alias = "sa")]
    ServiceAccount {
        /// Name of the service account to create
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Display one or many resources
    Get { resource: String },
    /// Display one or many resources
    Create {
        #[command(subcommand)]
        resource_kind: CreationKind,
    },
    /// Delete resources
    Delete { resource: String, item_name: String },
    /// Edit resource
    Edit { resource: String, item_name: String },
    /// Apply resource from file
    Apply { path: String },
    /// Print a nushell completion script to stdout
    Completions,
}

impl Commands {
    /// Generate and print the nushell completion script.
    pub fn print_completions() {
        clap_complete::generate(
            clap_complete_nushell::Nushell,
            &mut Cli::command(),
            "kubectl-lite",
            &mut std::io::stdout(),
        );
    }
}
