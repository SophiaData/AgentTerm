import { readFileSync } from "node:fs";

const read = (path) => readFileSync(path, "utf8");
const packageVersion = JSON.parse(read("package.json")).version;
const dependencies = JSON.parse(read("package.json")).dependencies;
const rustPackages = new Map(
  Array.from(
    read("src-tauri/Cargo.lock").matchAll(
      /\[\[package\]\]\r?\nname = "([^"]+)"\r?\nversion = "([^"]+)"/g,
    ),
    (match) => [match[1], match[2]],
  ),
);
for (const [name, version] of Object.entries(dependencies)) {
  if (!name.startsWith("@tauri-apps/plugin-")) continue;
  const crate = name.replace("@tauri-apps/", "tauri-");
  const rustVersion = rustPackages.get(crate);
  if (
    !rustVersion ||
    version.split(".").slice(0, 2).join(".") !==
      rustVersion.split(".").slice(0, 2).join(".")
  ) {
    console.error(
      `Tauri plugin version drift: ${name} ${version}, ${crate} ${rustVersion}`,
    );
    process.exit(1);
  }
}
const versions = [
  packageVersion,
  JSON.parse(read("src-tauri/tauri.conf.json")).version,
  /^version\s*=\s*"([^"]+)"/m.exec(read("src-tauri/Cargo.toml"))?.[1],
  /name = "agentterm"\r?\nversion = "([^"]+)"/.exec(
    read("src-tauri/Cargo.lock"),
  )?.[1],
  JSON.parse(read("npm/agentterm/package.json")).version,
  /^version\s*=\s*"([^"]+)"/m.exec(read("agentterm-tui/Cargo.toml"))?.[1],
  /name = "agentterm-tui"\r?\nversion = "([^"]+)"/.exec(
    read("agentterm-tui/Cargo.lock"),
  )?.[1],
];
if (versions.some((version) => version !== packageVersion)) {
  console.error(`Version drift: ${versions.join(", ")}`);
  process.exit(1);
}

for (const workflow of [
  ".github/workflows/quality.yml",
  ".github/workflows/packages.yml",
]) {
  for (const line of read(workflow).split(/\r?\n/)) {
    const match = /^\s*(?:-\s*)?uses:\s*([^#\s]+)/.exec(line);
    if (!match) continue;
    const reference = match[1];
    if (!/@[0-9a-f]{40}$/i.test(reference)) {
      console.error(
        `${workflow}: action is not pinned to a commit: ${reference}`,
      );
      process.exit(1);
    }
  }
}

const npm = JSON.parse(read("npm/agentterm/package.json"));
if (
  JSON.stringify(npm.os) !== JSON.stringify(["win32"]) ||
  JSON.stringify(npm.cpu) !== JSON.stringify(["x64"])
) {
  console.error("npm package must explicitly target win32 x64");
  process.exit(1);
}

// The launcher path and the npm packager's staged vendor name must agree,
// otherwise a package built from a renamed executable cannot start.
const launcher = read("npm/agentterm/bin/agentterm.js");
const packager = read("scripts/package-npm.mjs");
const launcherName = /join\(root,\s*"vendor",\s*"([^"]+)"\)/.exec(
  launcher,
)?.[1];
const stagedName = /join\(packageDir,\s*"vendor",\s*"([^"]+)"\)/.exec(
  packager,
)?.[1];
if (!launcherName || !stagedName || launcherName !== stagedName) {
  console.error(
    `npm vendor name mismatch: launcher=${launcherName}, packager=${stagedName}`,
  );
  process.exit(1);
}

// Release assembly must be able to carry both platforms' artifacts.
const release = read("scripts/package-release.mjs");
for (const token of [
  "windows-x64-setup.exe",
  "macos-arm64.dmg",
  "macos-arm64-tui",
]) {
  if (!release.includes(token)) {
    console.error(`release assembly is missing asset pattern: ${token}`);
    process.exit(1);
  }
}
