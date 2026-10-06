// Development ports of this project (infra/dev-ports.env), each overridable by the same-named
// environment variable: the same rule as ./dev (scripts/dev_ports.py).
import { existsSync, readFileSync } from "node:fs";

// Absent where only frontend/ is present (the release image's build stage): production builds
// need no development ports.
const file = new URL("../../infra/dev-ports.env", import.meta.url);
const declared = Object.fromEntries(
  (existsSync(file) ? readFileSync(file, "utf8") : "")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => line.split("=", 2).map((part) => part.trim())),
);

/** @param {string} name e.g. "DEV_API_PORT" @returns {number} */
export function devPort(name) {
  const value = process.env[name] || declared[name];
  if (!value) throw new Error(`${name} is not declared in infra/dev-ports.env`);
  return Number(value);
}
