use clap::Parser;

/// Generate YAML for any Kubernetes resource or CRD using cluster schema and smart defaults.
#[derive(Parser, Debug)]
#[command(name = "kubectl-ditto", version)]
pub struct Args {
    /// Resource type (e.g. deployment, svc, certificates.cert-manager.io)
    pub resource: Option<String>,

    /// Resource name
    pub name: Option<String>,

    /// Namespace for the resource (omit for cluster-scoped resources)
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Output only required fields (skip optional fields with defaults)
    #[arg(long)]
    pub minimal: bool,

    /// Include all optional fields with their defaults
    #[arg(long)]
    pub full: bool,

    /// Interactively prompt for required field values
    #[arg(short, long)]
    pub interactive: bool,

    /// Suppress description comments in output
    #[arg(long)]
    pub no_comments: bool,

    /// Dump the raw OpenAPI schema JSON for the resource (debug)
    #[arg(long)]
    pub dump_schema: bool,
}
