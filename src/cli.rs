use argh::FromArgs;

/// Generate YAML for any Kubernetes resource or CRD using cluster schema and smart defaults.
#[derive(FromArgs, Debug)]
pub struct Args {
    /// resource type (e.g. deployment, svc, certificates.cert-manager.io)
    #[argh(positional)]
    pub resource: String,

    /// resource name
    #[argh(positional)]
    pub name: Option<String>,

    /// namespace for the resource (omit for cluster-scoped resources)
    #[argh(option, short = 'n')]
    pub namespace: Option<String>,

    /// output only required fields (skip optional fields with defaults)
    #[argh(switch)]
    pub minimal: bool,

    /// include all optional fields with their defaults
    #[argh(switch)]
    pub full: bool,

    /// interactively prompt for required field values
    #[argh(switch, short = 'i')]
    pub interactive: bool,

    /// suppress description comments in output
    #[argh(switch)]
    pub no_comments: bool,

    /// dump the raw OpenAPI schema JSON for the resource (debug)
    #[argh(switch)]
    pub dump_schema: bool,
}
