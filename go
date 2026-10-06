#!/usr/bin/env bash
# Short entry point for the VectorCraft Cloud installer (easy to type in a web console):
#   curl -L raw.githubusercontent.com/milkpack/Claude/HEAD/go | bash
set -euo pipefail
curl -fsSL https://raw.githubusercontent.com/milkpack/Claude/HEAD/packaging/collab/deploy/install.sh | bash
