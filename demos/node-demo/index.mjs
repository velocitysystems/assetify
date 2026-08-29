import { createRequire } from "node:module";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// `npm run build` generates index.js (the addon loader) next to this file.
const require = createRequire(import.meta.url);
const { loadAsset } = require("./index.js");

const summary = await loadAsset(mkdtempSync(join(tmpdir(), "assetify-node-demo-")));
console.log(JSON.stringify(summary, null, 2));
