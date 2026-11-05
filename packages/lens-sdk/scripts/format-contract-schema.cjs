"use strict";

const fs = require("node:fs");
const path = require("node:path");

const schemaPath = path.resolve(__dirname, "../../../schemas/lenses/v1/lens-contract.schema.json");

function formatJson(value) {
  return `${formatValue(value, 0)}\n`;
}

function formatValue(value, depth) {
  if (Array.isArray(value)) {
    const inline = `[${value.map((item) => JSON.stringify(item)).join(", ")}]`;
    if (value.every(isPrimitive) && `${indent(depth)}${inline}`.length <= 120) {
      return inline;
    }
    if (value.length === 0) {
      return "[]";
    }
    const entries = value.map(
      (item) => `${indent(depth + 1)}${formatValue(item, depth + 1).trimStart()}`
    );
    return `[\n${entries.join(",\n")}\n${indent(depth)}]`;
  }
  if (value && typeof value === "object") {
    const entries = Object.entries(value);
    if (entries.length === 0) {
      return "{}";
    }
    const properties = entries.map(
      ([key, item]) =>
        `${indent(depth + 1)}${JSON.stringify(key)}: ${formatValue(item, depth + 1).trimStart()}`
    );
    return `{\n${properties.join(",\n")}\n${indent(depth)}}`;
  }
  return JSON.stringify(value);
}

function indent(depth) {
  return "  ".repeat(depth);
}

function isPrimitive(value) {
  return value === null || (typeof value !== "object" && typeof value !== "function");
}

if (require.main === module) {
  const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));
  fs.writeFileSync(schemaPath, formatJson(schema), "utf8");
}

module.exports = { formatJson };
