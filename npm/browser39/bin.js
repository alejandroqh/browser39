#!/usr/bin/env node
const { execFileSync } = require("child_process");
const { executablePath } = require("./index.js");

try {
  execFileSync(executablePath(), process.argv.slice(2), { stdio: "inherit" });
} catch (err) {
  if (err.status !== undefined) process.exit(err.status);
  console.error(err.message);
  process.exit(1);
}
