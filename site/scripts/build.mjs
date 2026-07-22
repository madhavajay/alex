import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const siteDirectory = path.resolve(scriptDirectory, "..");
const outputDirectory = path.join(siteDirectory, "dist");

await rm(outputDirectory, { recursive: true, force: true });
await mkdir(outputDirectory, { recursive: true });
await cp(path.join(siteDirectory, "src"), outputDirectory, {
  recursive: true,
  filter: (source) => path.basename(source) !== ".DS_Store"
});
await cp(path.join(siteDirectory, "old"), path.join(outputDirectory, "old"), { recursive: true });

console.log("Built Alex public site in site/dist");
