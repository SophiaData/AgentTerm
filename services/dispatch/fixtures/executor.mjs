import { writeFileSync } from "node:fs";
// A native-process fixture with the same completion envelope as Codex exec.
writeFileSync("src/value.txt", "done\n");
process.stdout.write(
  JSON.stringify({ type: "turn.completed", usage: {} }) + "\n",
);
