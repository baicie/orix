import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const version = process.argv[2];

if (!version) {
  console.error("Usage: node scripts/sync-version.mjs <version>");
  process.exit(1);
}

const packages = [
  "main",
  "darwin-arm64",
  "darwin-x64",
  "linux-x64-gnu",
  "linux-arm64-gnu",
  "linux-x64-musl",
  "win32-x64-msvc",
];

for (const dir of packages) {
  const file = path.join(root, "npm", dir, "package.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));

  json.version = version;

  if (dir === "main") {
    for (const name of Object.keys(json.optionalDependencies)) {
      json.optionalDependencies[name] = version;
    }
  }

  fs.writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`);
}

// Sync Cargo.toml workspace version and crate aliases
const cargoToml = path.join(root, "Cargo.toml");
const cargo = fs.readFileSync(cargoToml, "utf8");
const updatedCargo = cargo
  .replace(/^version = "[\d.]+"$/m, `version = "${version}"`)
  .replace(
    /^(orix-(?:cli|core|config|utils|domain|manifest|resolver|registry|fetcher|store|lockfile|linker|workspace) = \{ path = "[^"]+", version = )"[\d.]+"( })$/m,
    `$1"${version}"$2`
  );
fs.writeFileSync(cargoToml, updatedCargo);

console.log(`Synced versions to ${version}`);
