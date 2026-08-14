#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

node <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const sidecarPath = path.join(process.cwd(), ".bootstrap", "managed-files.json");
const sidecar = JSON.parse(fs.readFileSync(sidecarPath, "utf8"));

for (const [managedPath, entry] of Object.entries(sidecar.managedFiles ?? {})) {
  const filePath = path.join(process.cwd(), managedPath);
  if (!fs.existsSync(filePath)) {
    throw new Error(`Managed file is missing: ${managedPath}`);
  }
  const actual = crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
  if (actual !== entry.sha256) {
    throw new Error(`Managed-file hash mismatch: ${managedPath}`);
  }
}

console.log("Bootstrap managed-file sidecar hashes passed.");
NODE
