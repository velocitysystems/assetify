import { createRequire } from "node:module";
import { join } from "node:path";

// `npm run build` generates index.js (the addon loader) next to this file.
const require = createRequire(import.meta.url);
const { loadAsset } = require("./index.js");

// The shared fixture tree is the cache: read-only, served in place.
const summary = await loadAsset(join(import.meta.dirname, "..", "assets"));
console.log(JSON.stringify(summary, null, 2));
