"use strict";

const { isDeepStrictEqual } = require("node:util");

const SUPPORTED_SCHEMA_KEYWORDS = new Set([
  "$schema",
  "$id",
  "$defs",
  "$ref",
  "title",
  "description",
  "type",
  "const",
  "enum",
  "required",
  "properties",
  "additionalProperties",
  "items",
  "uniqueItems",
  "minItems",
  "maxItems",
  "minProperties",
  "minLength",
  "minimum",
  "maximum",
  "pattern",
  "oneOf",
  "anyOf",
  "allOf",
  "if",
  "then",
  "not"
]);

function assertSchemaSubsetSupported(rootSchema) {
  inspectSchema(rootSchema, "$");
}

function assertValidDefinition(rootSchema, definitionName, value) {
  const errors = validationErrorsForDefinition(rootSchema, definitionName, value);
  if (errors.length > 0) {
    throw new Error(
      `${definitionName} failed targeted schema validation:\n${errors.map((error) => `- ${error}`).join("\n")}`
    );
  }
}

function validationErrorsForDefinition(rootSchema, definitionName, value) {
  const definition = rootSchema.$defs?.[definitionName];
  if (!definition) {
    throw new Error(`Missing schema definition ${definitionName}`);
  }
  return validate(definition, value, rootSchema, "$", new Set());
}

function inspectSchema(schema, path) {
  if (typeof schema === "boolean") {
    return;
  }
  if (!isObject(schema)) {
    throw new Error(`Schema node ${path} must be an object or boolean`);
  }
  for (const key of Object.keys(schema)) {
    if (!SUPPORTED_SCHEMA_KEYWORDS.has(key)) {
      throw new Error(`Targeted schema validator does not implement ${key} at ${path}`);
    }
  }
  for (const [name, definition] of Object.entries(schema.$defs ?? {})) {
    inspectSchema(definition, `${path}.$defs.${name}`);
  }
  for (const [name, property] of Object.entries(schema.properties ?? {})) {
    inspectSchema(property, `${path}.properties.${name}`);
  }
  if (isObject(schema.additionalProperties)) {
    inspectSchema(schema.additionalProperties, `${path}.additionalProperties`);
  }
  for (const key of ["items", "not", "if", "then"]) {
    if (schema[key] !== undefined) {
      inspectSchema(schema[key], `${path}.${key}`);
    }
  }
  for (const key of ["oneOf", "anyOf", "allOf"]) {
    for (const [index, child] of (schema[key] ?? []).entries()) {
      inspectSchema(child, `${path}.${key}[${index}]`);
    }
  }
}

function validate(schema, value, rootSchema, path, resolvingRefs) {
  if (schema === true) {
    return [];
  }
  if (schema === false) {
    return [`${path} is forbidden by the schema`];
  }

  const errors = [];
  if (schema.$ref !== undefined) {
    const target = resolveLocalRef(rootSchema, schema.$ref);
    if (resolvingRefs.has(schema.$ref)) {
      throw new Error(`Recursive schema reference ${schema.$ref} is not supported`);
    }
    const nextRefs = new Set(resolvingRefs);
    nextRefs.add(schema.$ref);
    errors.push(...validate(target, value, rootSchema, path, nextRefs));
  }
  if (schema.type !== undefined && !matchesType(value, schema.type)) {
    errors.push(`${path} must have type ${schema.type}`);
  }
  if (schema.const !== undefined && !isDeepStrictEqual(value, schema.const)) {
    errors.push(`${path} must equal ${JSON.stringify(schema.const)}`);
  }
  if (schema.enum !== undefined && !schema.enum.some((item) => isDeepStrictEqual(value, item))) {
    errors.push(`${path} must be one of ${schema.enum.map(JSON.stringify).join(", ")}`);
  }

  if (typeof value === "string") {
    if (schema.minLength !== undefined && [...value].length < schema.minLength) {
      errors.push(`${path} must contain at least ${schema.minLength} characters`);
    }
    if (schema.pattern !== undefined && !new RegExp(schema.pattern, "u").test(value)) {
      errors.push(`${path} must match ${schema.pattern}`);
    }
  }
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) {
      errors.push(`${path} must be at least ${schema.minimum}`);
    }
    if (schema.maximum !== undefined && value > schema.maximum) {
      errors.push(`${path} must be at most ${schema.maximum}`);
    }
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) {
      errors.push(`${path} must contain at least ${schema.minItems} items`);
    }
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      errors.push(`${path} must contain at most ${schema.maxItems} items`);
    }
    if (schema.uniqueItems) {
      for (let left = 0; left < value.length; left += 1) {
        for (let right = left + 1; right < value.length; right += 1) {
          if (isDeepStrictEqual(value[left], value[right])) {
            errors.push(`${path} must contain unique items`);
          }
        }
      }
    }
    if (schema.items !== undefined) {
      value.forEach((item, index) => {
        errors.push(...validate(schema.items, item, rootSchema, `${path}[${index}]`, resolvingRefs));
      });
    }
  }
  if (isObject(value)) {
    const properties = schema.properties ?? {};
    if (schema.minProperties !== undefined && Object.keys(value).length < schema.minProperties) {
      errors.push(`${path} must contain at least ${schema.minProperties} properties`);
    }
    for (const required of schema.required ?? []) {
      if (!Object.hasOwn(value, required)) {
        errors.push(`${path} requires property ${required}`);
      }
    }
    for (const [key, propertySchema] of Object.entries(properties)) {
      if (Object.hasOwn(value, key)) {
        errors.push(
          ...validate(propertySchema, value[key], rootSchema, `${path}.${key}`, resolvingRefs)
        );
      }
    }
    for (const key of Object.keys(value)) {
      if (Object.hasOwn(properties, key)) {
        continue;
      }
      if (schema.additionalProperties === false) {
        errors.push(`${path} contains unknown property ${key}`);
      } else if (isObject(schema.additionalProperties)) {
        errors.push(
          ...validate(
            schema.additionalProperties,
            value[key],
            rootSchema,
            `${path}.${key}`,
            resolvingRefs
          )
        );
      }
    }
  }

  for (const child of schema.allOf ?? []) {
    errors.push(...validate(child, value, rootSchema, path, resolvingRefs));
  }
  if (schema.anyOf !== undefined) {
    const matches = schema.anyOf.filter(
      (child) => validate(child, value, rootSchema, path, resolvingRefs).length === 0
    ).length;
    if (matches === 0) {
      errors.push(`${path} must match at least one anyOf branch`);
    }
  }
  if (schema.oneOf !== undefined) {
    const matches = schema.oneOf.filter(
      (child) => validate(child, value, rootSchema, path, resolvingRefs).length === 0
    ).length;
    if (matches !== 1) {
      errors.push(`${path} must match exactly one oneOf branch; matched ${matches}`);
    }
  }
  if (schema.not !== undefined && validate(schema.not, value, rootSchema, path, resolvingRefs).length === 0) {
    errors.push(`${path} must not match the forbidden schema`);
  }
  if (
    schema.if !== undefined &&
    validate(schema.if, value, rootSchema, path, resolvingRefs).length === 0 &&
    schema.then !== undefined
  ) {
    errors.push(...validate(schema.then, value, rootSchema, path, resolvingRefs));
  }
  return errors;
}

function resolveLocalRef(rootSchema, reference) {
  const prefix = "#/$defs/";
  if (typeof reference !== "string" || !reference.startsWith(prefix)) {
    throw new Error(`Targeted schema validator supports only local $defs references: ${reference}`);
  }
  const name = reference.slice(prefix.length);
  const target = rootSchema.$defs?.[name];
  if (!target) {
    throw new Error(`Unresolved schema reference ${reference}`);
  }
  return target;
}

function matchesType(value, type) {
  switch (type) {
    case "object":
      return isObject(value);
    case "array":
      return Array.isArray(value);
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "integer":
      return Number.isInteger(value);
    case "boolean":
      return typeof value === "boolean";
    case "null":
      return value === null;
    default:
      throw new Error(`Targeted schema validator does not implement type ${type}`);
  }
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

module.exports = {
  assertSchemaSubsetSupported,
  assertValidDefinition,
  validationErrorsForDefinition
};
