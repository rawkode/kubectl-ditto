#!/usr/bin/env bash
# Update sha256 hashes in the krew plugin manifest after a release.
# Usage: ./update-sha256.sh v0.1.0
set -euo pipefail

VERSION="${1:?Usage: update-sha256.sh <version>}"
MANIFEST="$(dirname "$0")/plugin.yaml"
BASE_URL="https://github.com/rawkode/kubectl-ditto/releases/download/${VERSION}"

PLATFORMS=(
    "darwin-arm64"
    "darwin-amd64"
    "linux-arm64"
    "linux-amd64"
    "windows-amd64"
)

for platform in "${PLATFORMS[@]}"; do
    archive="kubectl-ditto-${platform}.tar.gz"
    url="${BASE_URL}/${archive}"
    echo "Fetching ${url}..."
    sha=$(curl -sL "${url}" | shasum -a 256 | awk '{print $1}')
    echo "  sha256: ${sha}"

    # Replace the PLACEHOLDER or existing sha256 for this platform's uri
    # Use awk to find the uri line and replace the next sha256 line
    awk -v uri="${url}" -v sha="${sha}" '
        $0 ~ uri { found=1 }
        found && /sha256:/ { sub(/"[^"]*"/, "\"" sha "\""); found=0 }
        { print }
    ' "${MANIFEST}" > "${MANIFEST}.tmp"
    mv "${MANIFEST}.tmp" "${MANIFEST}"
done

# Update version
sed -i.bak "s/version: v[0-9.]*/version: ${VERSION}/" "${MANIFEST}"
sed -i.bak "s|/v[0-9.]*/|/${VERSION}/|g" "${MANIFEST}"
rm -f "${MANIFEST}.bak"

echo "Done! Updated ${MANIFEST}"
