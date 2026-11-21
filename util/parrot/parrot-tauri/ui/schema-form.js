// SPDX-License-Identifier: Apache-2.0
//
// A pragmatic JSON-Schema-to-form renderer for the MCP tool playground.
// Supports: string (+ enum), number/integer, boolean, array of primitives,
// and one level of nested object -- deliberately not full JSON Schema.
// `oneOf`/`anyOf`/`$ref`/conditional schemas fall back to a raw-JSON
// textarea instead of being silently dropped or mis-rendered.

class FieldError extends Error {}

function isPrimitiveType(type) {
  return type === "string" || type === "number" || type === "integer" || type === "boolean";
}

function addHint(wrapper, text) {
  const hint = document.createElement("div");
  hint.className = "field-hint";
  hint.textContent = text;
  wrapper.appendChild(hint);
}

// Renders one field into `container`, returning a getter () => value
// (or `undefined` when empty and not required) that throws FieldError on
// an invalid or missing required value.
function renderField(container, name, schema, required) {
  const wrapper = document.createElement("div");
  wrapper.className = "field";

  const label = document.createElement("label");
  label.textContent = (schema.title || name) + (required ? " *" : "");
  wrapper.appendChild(label);

  if (schema.description) {
    addHint(wrapper, schema.description);
  }

  let getValue;

  if (Array.isArray(schema.enum)) {
    const select = document.createElement("select");
    if (!required) {
      const none = document.createElement("option");
      none.value = "";
      none.textContent = "(none)";
      select.appendChild(none);
    }
    for (const option of schema.enum) {
      const opt = document.createElement("option");
      opt.value = option;
      opt.textContent = String(option);
      select.appendChild(opt);
    }
    wrapper.appendChild(select);
    getValue = () => {
      if (select.value === "") {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      return select.value;
    };
  } else if (schema.type === "boolean") {
    const input = document.createElement("input");
    input.type = "checkbox";
    wrapper.appendChild(input);
    getValue = () => input.checked;
  } else if (schema.type === "number" || schema.type === "integer") {
    const input = document.createElement("input");
    input.type = "number";
    if (schema.type === "integer") input.step = "1";
    wrapper.appendChild(input);
    getValue = () => {
      if (input.value === "") {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      const n = Number(input.value);
      if (Number.isNaN(n)) throw new FieldError(`"${name}" must be a number`);
      return schema.type === "integer" ? Math.trunc(n) : n;
    };
  } else if (schema.type === "array" && schema.items && isPrimitiveType(schema.items.type)) {
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = "comma-separated values";
    wrapper.appendChild(input);
    getValue = () => {
      const raw = input.value.trim();
      if (raw === "") {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      const parts = raw
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s !== "");
      if (schema.items.type === "number" || schema.items.type === "integer") {
        return parts.map((p) => {
          const n = Number(p);
          if (Number.isNaN(n)) throw new FieldError(`"${name}" must be a list of numbers`);
          return schema.items.type === "integer" ? Math.trunc(n) : n;
        });
      }
      if (schema.items.type === "boolean") {
        return parts.map((p) => p === "true");
      }
      return parts;
    };
  } else if (schema.type === "object" && schema.properties) {
    const fieldset = document.createElement("fieldset");
    fieldset.className = "nested-object";
    const getters = buildFields(fieldset, schema.properties, schema.required || []);
    wrapper.appendChild(fieldset);
    getValue = () => {
      const obj = {};
      let any = false;
      for (const [key, getter] of Object.entries(getters)) {
        const v = getter();
        if (v !== undefined) {
          obj[key] = v;
          any = true;
        }
      }
      if (!any) {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      return obj;
    };
  } else if (schema.type === "string" || schema.type === undefined) {
    const input = document.createElement("input");
    input.type = "text";
    wrapper.appendChild(input);
    getValue = () => {
      if (input.value === "") {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      return input.value;
    };
  } else {
    // oneOf/anyOf/$ref/tuple-arrays/... -- unsupported shape, not silently
    // dropped: let the operator supply raw JSON instead.
    const textarea = document.createElement("textarea");
    textarea.placeholder = "raw JSON value";
    textarea.className = "raw-json-fallback";
    wrapper.appendChild(textarea);
    addHint(wrapper, "Unsupported schema shape for this field — enter raw JSON.");
    getValue = () => {
      const raw = textarea.value.trim();
      if (raw === "") {
        if (required) throw new FieldError(`"${name}" is required`);
        return undefined;
      }
      try {
        return JSON.parse(raw);
      } catch {
        throw new FieldError(`"${name}" must be valid JSON`);
      }
    };
  }

  container.appendChild(wrapper);
  return getValue;
}

function buildFields(container, properties, requiredList) {
  const getters = {};
  for (const [name, schema] of Object.entries(properties || {})) {
    getters[name] = renderField(container, name, schema, requiredList.includes(name));
  }
  return getters;
}

// Renders `schema` (a tool's inputSchema) into `container`. Returns
// `{ collect() }`, where `collect()` returns `{ ok: true, values }` or
// `{ ok: false, error }`.
export function renderToolForm(container, schema) {
  container.innerHTML = "";

  const properties = schema && schema.properties;
  if (!properties || Object.keys(properties).length === 0) {
    addHint(container, "This tool takes no arguments.");
    return { collect: () => ({ ok: true, values: {} }) };
  }

  const getters = buildFields(container, properties, schema.required || []);
  return {
    collect() {
      const values = {};
      try {
        for (const [name, getter] of Object.entries(getters)) {
          const v = getter();
          if (v !== undefined) values[name] = v;
        }
      } catch (err) {
        if (err instanceof FieldError) return { ok: false, error: err.message };
        throw err;
      }
      return { ok: true, values };
    },
  };
}
