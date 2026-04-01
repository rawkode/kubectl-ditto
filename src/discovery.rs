use anyhow::{Result, bail};
use kube::Client;
use serde::Deserialize;

/// A fully resolved Kubernetes resource type.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    pub api_resource: ResolvedApiResource,
    pub namespaced: bool,
    pub group: String,
    pub version: String,
}

/// Minimal API resource info we need.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolvedApiResource {
    pub kind: String,
    pub plural: String,
}

/// An entry from the API server's resource list.
#[derive(Debug, Clone, Deserialize)]
struct ApiResourceEntry {
    name: String,
    #[serde(rename = "singularName", default)]
    singular_name: String,
    #[serde(default)]
    namespaced: bool,
    kind: String,
    #[serde(rename = "shortNames", default)]
    short_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ApiResourceList {
    resources: Vec<ApiResourceEntry>,
}

#[derive(Debug, Deserialize)]
struct ApiGroupList {
    groups: Vec<ApiGroupEntry>,
}

#[derive(Debug, Deserialize)]
struct ApiGroupEntry {
    name: String,
    versions: Vec<ApiGroupVersion>,
}

#[derive(Debug, Deserialize)]
struct ApiGroupVersion {
    #[serde(rename = "groupVersion")]
    group_version: String,
    version: String,
}

/// A collected resource with all its metadata from the API server.
#[derive(Debug, Clone)]
struct DiscoveredResource {
    entry: ApiResourceEntry,
    group: String,
    version: String,
}

/// Resolve a user-provided resource string to a concrete API resource.
///
/// Queries the API server directly for resource lists, which include short names.
///
/// Supports:
///   - Plural names: "deployments", "services"
///   - Singular names: "deployment", "service"
///   - Short names: "deploy", "svc", "cm" (from API server, not hardcoded)
///   - Fully qualified: "certificates.cert-manager.io"
///   - Kind names: "Deployment", "Service"
pub async fn resolve_resource(client: &Client, input: &str) -> Result<ResolvedResource> {
    let input_lower = input.to_lowercase();

    // Check if input contains a dot — could be "resource.group" format
    let (search_name, search_group) = if let Some(dot_pos) = input_lower.find('.') {
        let name = &input_lower[..dot_pos];
        let group = &input_lower[dot_pos + 1..];
        (Some(name.to_string()), Some(group.to_string()))
    } else {
        (None, None)
    };

    // Discover all resources from the API server
    let all_resources = discover_all_resources(client).await?;

    for res in &all_resources {
        // If user specified a group, filter to it
        if let Some(ref sg) = search_group
            && res.group.to_lowercase() != *sg
        {
            continue;
        }

        let is_match = match (&search_name, &search_group) {
            // Fully qualified: "certificates.cert-manager.io"
            (Some(name), Some(_)) => {
                res.entry.name.to_lowercase() == *name
                    || res.entry.kind.to_lowercase() == *name
                    || res.entry.singular_name.to_lowercase() == *name
            }
            // Simple name, short name, or kind
            _ => {
                res.entry.name.to_lowercase() == input_lower
                    || res.entry.kind.to_lowercase() == input_lower
                    || res.entry.singular_name.to_lowercase() == input_lower
                    || res
                        .entry
                        .short_names
                        .iter()
                        .any(|s| s.to_lowercase() == input_lower)
            }
        };

        if is_match {
            // Skip sub-resources (e.g. "deployments/status", "pods/log")
            if res.entry.name.contains('/') {
                continue;
            }

            return Ok(ResolvedResource {
                api_resource: ResolvedApiResource {
                    kind: res.entry.kind.clone(),
                    plural: res.entry.name.clone(),
                },
                namespaced: res.entry.namespaced,
                group: res.group.clone(),
                version: res.version.clone(),
            });
        }
    }

    // Collect near-matches to help debug lookup failures
    let mut suggestions: Vec<String> = Vec::new();
    for res in &all_resources {
        if res.entry.name.contains('/') {
            continue;
        }
        let names: Vec<&str> = std::iter::once(res.entry.name.as_str())
            .chain(std::iter::once(res.entry.kind.as_str()))
            .chain(std::iter::once(res.entry.singular_name.as_str()))
            .chain(res.entry.short_names.iter().map(|s| s.as_str()))
            .collect();
        if names.iter().any(|n| {
            n.to_lowercase().contains(&input_lower) || input_lower.contains(&n.to_lowercase())
        }) {
            suggestions.push(format!(
                "  {} (kind: {}, shortNames: [{}])",
                res.entry.name,
                res.entry.kind,
                res.entry.short_names.join(", ")
            ));
        }
    }

    if suggestions.is_empty() {
        bail!(
            "Could not find resource '{}' in the cluster ({} resources discovered). \
             Make sure the CRD is installed and you have access.",
            input,
            all_resources.len()
        );
    } else {
        bail!(
            "Could not find resource '{}' in the cluster, but found similar resources:\n{}\n\
             Use the full resource name or a listed shortName.",
            input,
            suggestions.join("\n")
        );
    }
}

/// Query all API groups and their resources from the API server.
/// This gives us short names, singular names, and all metadata.
async fn discover_all_resources(client: &Client) -> Result<Vec<DiscoveredResource>> {
    let mut all = Vec::new();

    // 1. Core API (v1): pods, services, configmaps, etc.
    let core_list: ApiResourceList = client
        .request(
            http::Request::builder()
                .uri("/api/v1")
                .body(Default::default())?,
        )
        .await?;

    for entry in core_list.resources {
        all.push(DiscoveredResource {
            entry,
            group: String::new(),
            version: "v1".to_string(),
        });
    }

    // 2. All other API groups
    let groups: ApiGroupList = client
        .request(
            http::Request::builder()
                .uri("/apis")
                .body(Default::default())?,
        )
        .await?;

    for group in &groups.groups {
        // Query all served versions — some resources only exist in non-preferred versions
        // (e.g. kyverno.io/v1 has "policies" but kyverno.io/v2 does not)
        let mut seen_resources = std::collections::HashSet::new();

        for gv_info in &group.versions {
            let uri = format!("/apis/{}", gv_info.group_version);

            let resource_list: Result<ApiResourceList, _> = client
                .request(
                    http::Request::builder()
                        .uri(&uri)
                        .body(Default::default())?,
                )
                .await;

            match resource_list {
                Ok(resource_list) => {
                    for entry in resource_list.resources {
                        // Deduplicate: only keep the first version of each resource
                        if seen_resources.insert(entry.name.clone()) {
                            all.push(DiscoveredResource {
                                entry,
                                group: group.name.clone(),
                                version: gv_info.version.clone(),
                            });
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: failed to discover resources for {}: {}",
                        gv_info.group_version, e
                    );
                }
            }
        }
    }

    Ok(all)
}
