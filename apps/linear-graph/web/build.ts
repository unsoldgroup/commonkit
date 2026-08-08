import { cp, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";

const root = import.meta.dir;
const dist = join(root, "dist");
await rm(dist, { recursive: true, force: true });
await mkdir(join(dist, "assets"), { recursive: true });
const result = await Bun.build({ entrypoints: [join(root, "src/app.ts")], outdir: join(dist, "assets"), minify: true, sourcemap: "linked", target: "browser" });
if (!result.success) throw new AggregateError(result.logs, "Linear graph web build failed");
await cp(join(root, "public/index.html"), join(dist, "index.html"));
await cp(join(root, "public/styles.css"), join(dist, "styles.css"));
