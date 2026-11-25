import { runPython } from "./python.mjs";

for (const script of ["scripts/verify_project.py", "scripts/run_quality.py"]) {
  const status = runPython([script]);
  if (status !== 0) process.exit(status);
}
