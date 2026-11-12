// Fail when desktop/src/generated is stale relative to the Rust types.
//
// Runs in CI after `scripts/generate-types.sh`; a deliberate Rust rename
// regenerates the file and leaves a diff, which this reports (ADR-0040).
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const generated = join("desktop", "src", "generated");

const status = execFileSync("git", ["status", "--porcelain", "--", generated], {
  cwd: root,
  encoding: "utf8",
}).trim();

if (status) {
  console.error(
    "desktop/src/generated is out of date. Run desktop/scripts/generate-types.sh and commit the result.\n",
  );
  console.error(status);
  process.exit(1);
}
console.log("desktop/src/generated is up to date");
