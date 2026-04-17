use std::path::PathBuf;

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

    /// Path to the kubeconfig file to use
    #[arg(long)]
    pub kubeconfig: Option<PathBuf>,

    /// Name of the kubeconfig context to use
    #[arg(long)]
    pub context: Option<String>,

    /// Output only required fields (skip optional fields with defaults)
    #[arg(long)]
    pub minimal: bool,

    /// Include all optional fields (with defaults in output, or prompts with -i)
    #[arg(long, alias = "all-fields")]
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

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn test_parses_kubeconfig_and_context_flags() {
        let args = Args::parse_from([
            "kubectl-ditto",
            "--kubeconfig",
            "/tmp/kubeconfig",
            "--context",
            "production",
            "deployment",
            "my-app",
        ]);

        assert_eq!(args.kubeconfig, Some(PathBuf::from("/tmp/kubeconfig")));
        assert_eq!(args.context.as_deref(), Some("production"));
        assert_eq!(args.resource.as_deref(), Some("deployment"));
        assert_eq!(args.name.as_deref(), Some("my-app"));
    }

    #[test]
    fn test_parses_context_without_custom_kubeconfig() {
        let args = Args::parse_from(["kubectl-ditto", "--context", "staging", "deployment"]);

        assert_eq!(args.kubeconfig, None);
        assert_eq!(args.context.as_deref(), Some("staging"));
        assert_eq!(args.resource.as_deref(), Some("deployment"));
    }
}
