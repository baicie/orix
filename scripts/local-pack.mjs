import fs from "node:fs";
import path from "node:path";
import { execSync } from "node:child_process";

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

for (const dir of packages) {
  const pkgDir = path.join(root, "npm", dir);
  const packageJson = path.join(pkgDir, "package.json");
  const json = JSON.parse(fs.readFileSync(packageJson, "utf8"));

  console.log(`Packing ${json.name}@${json.version}...`);

  execSync("npm pack", {
    cwd: pkgDir,
    stdio: "inherit",
  });

  console.log(`  -> ${json.name} packed successfully`);
}

console.log("\nAll packages packed. Local tgz files created in each npm/<dir>/ directory.");
console.log(
  "\nTo test locally:\n" +
    "  npm i -g ./npm/main/baicie-orix-*.tgz\n" +
    "  orix --version",
);
