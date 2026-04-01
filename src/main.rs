mod cli;
mod discovery;
mod generate;
mod interactive;
mod schema;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: cli::Args = argh::from_env();
    let client = kube::Client::try_default().await?;

    // 1. Resolve the resource type (dynamic short names from API server)
    let resolved = discovery::resolve_resource(&client, &args.resource).await?;

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
