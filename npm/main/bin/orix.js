#!/usr/bin/env node

"use strict";

const { spawnSync } = require("node:child_process");

function isMusl() {
  if (process.platform !== "linux") return false;

  try {
    const report =
      process.report && process.report.getReport
        ? process.report.getReport()
        : null;

    const glibcVersionRuntime =
      report && report.header && report.header.glibcVersionRuntime;

    return !glibcVersionRuntime;
  } catch {
    return false;
  }
}

function getNativePackageName() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "arm64") {
    return "@orix/orix-darwin-arm64";
  }

  if (platform === "darwin" && arch === "x64") {
    return "@orix/orix-darwin-x64";
  }

  if (platform === "win32" && arch === "x64") {
    return "@orix/orix-win32-x64-msvc";
  }

  if (platform === "linux" && arch === "x64") {
    return isMusl()
      ? "@orix/orix-linux-x64-musl"
      : "@orix/orix-linux-x64-gnu";
  }

  if (platform === "linux" && (arch === "arm64" || arch === "aarch64")) {
    return "@orix/orix-linux-arm64-gnu";
  }

  throw new Error(
    `Unsupported platform: ${platform} ${arch}. ` +
      `Please open an issue with your OS and CPU info.`,
  );
}

function getBinaryPath() {
  const pkg = getNativePackageName();
  const binName = process.platform === "win32" ? "orix.exe" : "orix";

  try {
    return require.resolve(`${pkg}/bin/${binName}`);
  } catch (error) {
    const message = [
      `Failed to resolve native binary package: ${pkg}`,
      "",
      "Possible reasons:",
      "1. optionalDependencies were disabled during install.",
      "2. The package manager skipped platform-specific packages.",
      "3. The installation cache is corrupted.",
      "",
      "Try reinstalling:",
      "  npm i -g @orix/orix",
      "  pnpm add -g @orix/orix",
      "",
      `Original error: ${error && error.message ? error.message : String(error)}`,
    ].join("\n");

    throw new Error(message, { cause: error });
  }
}

const binPath = getBinaryPath();

const result = spawnSync(binPath, process.argv.slice(2), {
  stdio: "inherit",
  cwd: process.cwd(),
  env: process.env,
  windowsHide: false,
});

if (result.error) {
  throw result.error;
}

if (typeof result.status === "number") {
  process.exit(result.status);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(1);
