// Post-build step (`npm run build`): prepare dist/ for serving without a compressing proxy.
// - Writes brotli (quality 11) and gzip (level 9) siblings for text files. The Rust server's static
//   handler negotiates them (ServeDir precompressed_br/gzip), so compression costs nothing per
//   request and uses levels too slow to apply on the fly.
// - Moves source maps out of dist/ into sourcemaps/ (for an error tracker's upload): the build
//   emits them "hidden" (no reference in the bundles), but anything left in dist/ is public.
import { mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

const dist = "dist";
const maps = "sourcemaps";
const TEXT = /\.(js|mjs|css|html|svg|json|txt|xml|webmanifest)$/;
const MIN_BYTES = 1024; // below this, the framing overhead outweighs the saving

const walk = (dir) =>
  readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });

rmSync(maps, { recursive: true, force: true }); // only this build's maps
let raw = 0;
let br = 0;
let moved = 0;
for (const file of walk(dist)) {
  if (file.endsWith(".map")) {
    const target = join(maps, relative(dist, file));
    mkdirSync(dirname(target), { recursive: true });
    renameSync(file, target);
    moved++;
    continue;
  }
  if (!TEXT.test(file)) continue;
  const body = readFileSync(file);
  if (body.length < MIN_BYTES) continue;
  const b = brotliCompressSync(body, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } });
  const g = gzipSync(body, { level: 9 });
  // Keep a variant only when it is smaller; otherwise the server sends the original.
  if (b.length < body.length) writeFileSync(`${file}.br`, b);
  if (g.length < body.length) writeFileSync(`${file}.gz`, g);
  raw += body.length;
  br += Math.min(b.length, body.length);
}
console.log(`precompress: ${raw} → ${br} bytes with brotli; ${moved} source maps moved to ${maps}/`);
