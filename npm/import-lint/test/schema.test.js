"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const packageDir = path.join(__dirname, "..");

test("the npm package includes a valid configuration schema", () => {
  const schemaPath = path.join(packageDir, "config.schema.json");
  const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));
  const packageJson = JSON.parse(
    fs.readFileSync(path.join(packageDir, "package.json"), "utf8"),
  );

  assert.equal(schema.title, "ImportLint configuration");
  assert.equal(schema.type, "object");
  assert.equal(schema.properties.$schema.type, "string");
  assert.ok(schema.properties.rules);
  assert.ok(schema.definitions.packageAccessRule.properties.nonTsFiles);
  assert.ok(packageJson.files.includes("config.schema.json"));
});
