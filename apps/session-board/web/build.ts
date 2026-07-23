import { cp, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";

const root = import.meta.dir;
const dist = join(root, "dist");
await rm(dist, { recursive: true, force: true });
await mkdir(join(dist, "assets"), { recursive: true });
const result = await Bun.build({ entrypoints: [join(root, "src/app.ts")], outdir: join(dist, "assets"), minify: true, sourcemap: "linked", target: "browser" });
if (!result.success) throw new AggregateError(result.logs, "Session Board web build failed");
for (const file of ["index.html", "styles.css", "manifest.webmanifest", "sw.js", "icon.svg", "icon-maskable.svg"]) await cp(join(root, "public", file), join(dist, file));
