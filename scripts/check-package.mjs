import fs from "node:fs";
import path from "node:path";

const root = process.cwd();

const packages = [
  "main",
  "darwin-arm64",
  "darwin-x64",
  "linux-x64-gnu",
  "linux-arm64-gnu",
  "linux-x64-musl",
  "win32-x64-msvc",
];

const versions = new Set();

for (const dir of packages) {
  const file = path.join(root, "npm", dir, "package.json");
  const json = JSON.parse(fs.readFileSync(file, "utf8"));

  versions.add(json.version);

  if (json.private) {
    throw new Error(`${dir} must not be private`);
  }

  if (!json.publishConfig || json.publishConfig.access !== "public") {
    throw new Error(`${dir} missing publishConfig.access=public`);
  }

  if (dir !== "main") {
    if (!json.files || !json.files.includes("bin")) {
      throw new Error(`${dir} must include bin in files`);
    }
  }
}

if (versions.size !== 1) {
  throw new Error(
    `Package versions are inconsistent: ${[...versions].join(", ")}`,
  );
}

console.log("Package metadata check passed");
