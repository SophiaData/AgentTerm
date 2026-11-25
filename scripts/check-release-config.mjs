import { readFileSync } from "node:fs";

const read = (path) => readFileSync(path, "utf8");
const packageVersion = JSON.parse(read("package.json")).version;
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
