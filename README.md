# kubectl-ditto

A kubectl plugin that generates YAML for **any** Kubernetes resource or CRD using the cluster's OpenAPI schema and smart defaults.

## Features

- **Any resource** — works with built-in resources and any installed CRD
- **Dynamic short names** — queries the API server directly, no hardcoded aliases
- **OpenAPI v3 + v2** — tries v3 first (better `oneOf`/`anyOf` support), falls back to v2
- **Schema comments** — field descriptions from the OpenAPI spec are emitted as YAML comments
- **Interactive mode** — prompts for required field values with type-aware inputs (selects for enums, booleans, key=value for maps)
- **Smart defaults** — enum first values, format-aware placeholders, commonly-needed fields included by default

## Usage

```bash
# Generate a Deployment
kubectl ditto deployment -n my-namespace my-app

# Generate a CRD resource
kubectl ditto certificates.cert-manager.io -n my-namespace my-cert

# Interactive mode — prompts for values
kubectl ditto deployment -n my-namespace my-app -i

# Minimal output (required fields only)
kubectl ditto deployment -n my-namespace my-app --minimal

# Full output (all fields with defaults)
kubectl ditto deployment -n my-namespace my-app --full

# Suppress description comments
kubectl ditto deployment -n my-namespace my-app --no-comments
```

## Install

### From source

```bash
cargo install --path .
```

### Via Krew

```bash
kubectl krew install ditto
```

The binary `kubectl-ditto` is placed on your PATH. kubectl automatically discovers plugins named `kubectl-<subcommand>`.

## How it works

1. **Discovery** — queries `/api/v1` and `/apis` to resolve your input against all resources in the cluster, including dynamic short names from the API server (no hardcoded list)
2. **Schema** — fetches the OpenAPI v3 spec (per group/version from `/openapi/v3`), falling back to v2 (`/openapi/v2`) for older clusters. Resolves `$ref` pointers and parses `oneOf`/`anyOf` variants
3. **Generate** — walks the structured schema to produce YAML with:
   - Field descriptions as inline `# comments`
   - Smart type-aware defaults
   - `--minimal` for required-only, `--full` for everything
   - `--interactive` for prompted input with enum selects, boolean confirms, and map entry

## Requirements

- A running Kubernetes cluster (uses your current kubeconfig context)
- CRDs must be installed for custom resources

## Release

To publish a new version:

1. Tag: `git tag v0.2.0 && git push --tags`
2. Build release binaries for each platform
3. Update Krew manifest hashes: `./deploy/krew/update-sha256.sh v0.2.0`
4. Submit PR to [krew-index](https://github.com/kubernetes-sigs/krew-index)
