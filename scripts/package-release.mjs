import {
  copyFileSync,
  createReadStream,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { createHash } from "node:crypto";

function option(name) {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

const installer = option("installer");
const desktop = option("desktop");
const tui = option("tui");
const npmPackage = option("npm");
const output = option("output");
if (!installer || !desktop || !tui || !npmPackage || !output) {
  console.error(
    "usage: node scripts/package-release.mjs --installer <exe> --desktop <exe> --tui <exe> --npm <tgz> --output <dir>",
  );
  process.exit(2);
}

const version = JSON.parse(readFileSync("package.json", "utf8")).version;
mkdirSync(output, { recursive: true });
const assets = [
  [resolve(installer), `AgentTerm-${version}-windows-x64-setup.exe`],
  [resolve(desktop), `agentterm-${version}-windows-x64.exe`],
  [resolve(tui), `agentterm-${version}-windows-x64-tui.exe`],
  [resolve(npmPackage), `agentterm-${version}.tgz`],
].map(([source, name]) => {
  if (!statSync(source).isFile())
    throw new Error(`Missing release input: ${source}`);
  const destination = join(resolve(output), name);
  copyFileSync(source, destination);
  return destination;
});

async function sha256(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

const entries = [];
for (const path of assets) {
  entries.push({
    name: basename(path),
    bytes: statSync(path).size,
    sha256: await sha256(path),
  });
}
entries.sort((left, right) => left.name.localeCompare(right.name));
writeFileSync(
  join(resolve(output), "SHA256SUMS.txt"),
  `${entries.map((entry) => `${entry.sha256}  ${entry.name}`).join("\n")}\n`,
);
writeFileSync(
  join(resolve(output), "release-manifest.json"),
  `${JSON.stringify({ version, tag: `v${version}`, assets: entries }, null, 2)}\n`,
);
console.log(`Release assets prepared in ${resolve(output)}`);
