// Bundle budget gate: gzip size of the JS/CSS needed for first load and of the largest route
// chunk. Budgets live in bundle-budget.json; raise them deliberately (with a reason) in review.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const dir = "dist/assets";
const budget = JSON.parse(readFileSync("bundle-budget.json", "utf8"));
const files = readdirSync(dir).filter((f) => /\.(js|css)$/.test(f));
const gz = (f) => gzipSync(readFileSync(`${dir}/${f}`)).length;
const html = readFileSync("dist/index.html", "utf8");
const initial = files.filter((f) => html.includes(`/assets/${f}`));
const initialBytes = initial.reduce((n, f) => n + gz(f), 0);
const routeChunks = files.filter((f) => f.endsWith(".js") && !initial.includes(f));
const largestRoute = Math.max(0, ...routeChunks.map(gz));
const totalBytes = files.reduce((n, f) => n + gz(f), 0);
const report = { initial_gzip_bytes: initialBytes, largest_route_gzip_bytes: largestRoute, total_gzip_bytes: totalBytes, files: files.length };
writeFileSync("dist/bundle-report.json", `${JSON.stringify(report, null, 2)}\n`);
console.log(report);
const fails = Object.entries(budget.max).filter(([k, max]) => report[k] > max).map(([k, max]) => `${k}: ${report[k]} > ${max}`);
if (fails.length) {
  console.error(`bundle budget exceeded:\n  ${fails.join("\n  ")}`);
  process.exit(1);
}
