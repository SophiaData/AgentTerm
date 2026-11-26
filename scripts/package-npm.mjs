import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const executable = resolve(
  process.argv[2] ?? "src-tauri/target/release/agentterm.exe",
);
const outputDir = resolve(process.argv[3] ?? "dist-npm");
if (!process.argv[2]) {
  console.error(
    "usage: node scripts/package-npm.mjs <desktop-exe> [output-dir]",
  );
  process.exit(2);
}
if (process.platform !== "win32" || !executable.endsWith(".exe")) {
  console.error(
    "This npm package currently supports the Windows x64 desktop build only.",
  );
  process.exit(2);
}
const size = statSync(executable).size;
if (size < 5 * 1024 * 1024) {
  console.error(
    `Desktop executable is unexpectedly small (${size} bytes): ${executable}`,
  );
  process.exit(1);
}

const sourceDir = resolve("npm/agentterm");
const packageDir = join(outputDir, "agentterm");
mkdirSync(join(packageDir, "vendor"), { recursive: true });
mkdirSync(join(packageDir, "bin"), { recursive: true });
copyFileSync(join(sourceDir, "README.md"), join(packageDir, "README.md"));
copyFileSync(
  join(sourceDir, "bin", "agentterm.js"),
  join(packageDir, "bin", "agentterm.js"),
);
copyFileSync(resolve("LICENSE"), join(packageDir, "LICENSE"));
// The launcher always starts vendor/agentterm.exe, so normalize the staged
// name regardless of which input path CI passes in.
copyFileSync(executable, join(packageDir, "vendor", "agentterm.exe"));

const manifest = JSON.parse(
  readFileSync(join(sourceDir, "package.json"), "utf8"),
);
writeFileSync(
  join(packageDir, "package.json"),
  `${JSON.stringify(
    {
      ...manifest,
      os: ["win32"],
      cpu: ["x64"],
    },
    null,
    2,
  )}\n`,
);

const packed = spawnSync(
  "pnpm",
  ["--dir", packageDir, "pack", "--pack-destination", outputDir],
  { stdio: "inherit", windowsHide: true, shell: process.platform === "win32" },
);
if (packed.error) throw packed.error;
process.exit(packed.status ?? 1);
