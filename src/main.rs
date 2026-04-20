mod cli;
mod discovery;
mod generate;
mod interactive;
mod schema;

use anyhow::Result;
use clap::Parser;
use kube::config::{KubeConfigOptions, Kubeconfig};

async fn client_for_args(args: &cli::Args) -> Result<kube::Client> {
    if args.kubeconfig.is_none() && args.context.is_none() {
        return Ok(kube::Client::try_default().await?);
    }

    let options = KubeConfigOptions {
        context: args.context.clone(),
        ..Default::default()
    };

    let config = if let Some(path) = &args.kubeconfig {
        let kubeconfig = Kubeconfig::read_from(path)?;
        kube::Config::from_custom_kubeconfig(kubeconfig, &options).await?
    } else {
        kube::Config::from_kubeconfig(&options).await?
    };

    Ok(kube::Client::try_from(config)?)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();

    let resource = match &args.resource {
        Some(r) => r.clone(),
        None => {
            cli::Args::parse_from(["kubectl-ditto", "--help"]);
            unreachable!()
        }
    };

    let client = client_for_args(&args).await?;

    // 1. Resolve the resource type (dynamic short names from API server)
    let resolved = discovery::resolve_resource(&client, &resource).await?;

    // 2. Dump raw schema if requested (debug)
    if args.dump_schema {
        let raw = schema::fetch_raw_schema(&client, &resolved).await?;
        println!("{}", serde_json::to_string_pretty(&raw)?);
        return Ok(());
    }

    // 3. Fetch the OpenAPI schema (tries v3 first, falls back to v2)
    let resource_schema = schema::fetch_schema(&client, &resolved).await?;

    // 4. Generate YAML with smart defaults, comments, and optional interactivity
    let yaml = generate::generate_yaml(&resolved, &resource_schema, &args)?;

    println!("{yaml}");
    Ok(())
}
