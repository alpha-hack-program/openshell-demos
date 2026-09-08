// Single esbuild pass: bundles the Preact app and copies index.html
// alongside it into dist/, which audit-dashboard's Rust binary serves
// directly via tower_http::services::ServeDir (see ../src/main.rs). No
// dev server, no watch mode — this is a demo dashboard, rebuilt as part
// of the Containerfile's frontend-builder stage, not iterated on live.
const esbuild = require("esbuild");
const fs = require("fs");

fs.mkdirSync("dist", { recursive: true });

esbuild
  .build({
    entryPoints: ["src/main.jsx"],
    bundle: true,
    outfile: "dist/bundle.js",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    minify: true,
    target: ["es2020"],
  })
  .then(() => {
    fs.copyFileSync("index.html", "dist/index.html");
    console.log("built dist/bundle.js + dist/index.html");
  })
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });
