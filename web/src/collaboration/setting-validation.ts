import type {
  SettingRequestForm,
  SettingConstraint,
} from "../generated/models";

import zoneNames from "./setting-time-zones.txt?raw";

const timeZones = new Set(zoneNames.trim().split("\n"));

export function formTitle(form: SettingRequestForm) {
  return form.kind === "text"
    ? form.fields.map((field) => field.label).join(" · ")
    : form.label;
}

async function validateText(
  input: string,
  rule: SettingConstraint | null | undefined,
  ja: boolean,
) {
  const value = input.replace(/\p{Cc}/gu, "");
  if (!value || Array.from(value).length > 2000)
    throw Error(
      ja ? "1〜2,000文字で入力してください。" : "Enter 1 to 2,000 characters.",
    );
  if (!rule) return value;
  if (rule.type === "regex") {
    // Use the same Unicode classes, flags and matching semantics as the server.
    const { init, RRegex } = await import("rregex/web");
    const { default: wasmUrl } = await import("rregex/lib/rregex.wasm?url");
    await init({ module_or_path: wasmUrl });
    const regex = new RRegex(rule.pattern);
    try {
      if (!regex.isMatch(value)) throw Error(rule.message);
    } finally {
      regex.free();
    }
  } else if (rule.type === "number") {
    const n = Number(value);
    if (
      !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(value) ||
      !Number.isFinite(n) ||
      n < rule.min ||
      n > rule.max ||
      (rule.integer && !Number.isInteger(n))
    )
      throw Error(
        ja
          ? `${rule.min}〜${rule.max} ${rule.unit} の範囲で${rule.integer ? "整数" : "数値"}を入力してください。`
          : `Enter a ${rule.integer ? "whole number" : "number"} from ${rule.min} to ${rule.max} ${rule.unit}.`,
      );
  } else {
    switch (rule.format) {
      case "email":
        if (
          !/^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(value) ||
          /(^\.|\.\.|\.@)/.test(value) ||
          !value
            .split("@")[1]
            .split(".")
            .every((part) => /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/i.test(part))
        )
          throw Error(
            ja
              ? "メールアドレスを入力してください。"
              : "Enter an email address.",
          );
        break;
      case "https_url":
        if (
          !value.startsWith("https://") ||
          /\s/u.test(value) ||
          !URL.canParse(value) ||
          !new URL(value).hostname
        )
          throw Error(
            ja
              ? "https:// で始まる URL を入力してください。"
              : "Enter an HTTPS URL.",
          );
        break;
      case "date": {
        const parsed = new Date(value + "T00:00:00Z");
        if (
          !/^\d{4}-\d{2}-\d{2}$/.test(value) ||
          Number.isNaN(parsed.valueOf()) ||
          parsed.toISOString().slice(0, 10) !== value
        )
          throw Error(
            ja
              ? "日付を YYYY-MM-DD で入力してください。"
              : "Enter a date as YYYY-MM-DD.",
          );
        break;
      }
      case "time":
        if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(value))
          throw Error(
            ja ? "時刻を HH:MM で入力してください。" : "Enter a time as HH:MM.",
          );
        break;
      case "time_zone":
        if (!timeZones.has(value)) {
          throw Error(
            ja
              ? "Asia/Tokyo などのタイムゾーンを入力してください。"
              : "Enter an IANA time zone such as Asia/Tokyo.",
          );
        }
    }
  }
  return value;
}

export async function answerValue(
  form: SettingRequestForm,
  data: FormData,
  ja: boolean,
) {
  if (form.kind === "secret") {
    const secret = String(data.get("secret") ?? "");
    if (
      !secret ||
      new TextEncoder().encode(secret).length > 8192 ||
      /[\p{Cc}\u2028\u2029]/u.test(secret)
    )
      throw Error(
        ja
          ? "秘密情報は制御文字や改行を含めず、8 KB 以内で入力してください。"
          : "Enter a single-line secret of up to 8 KB without control characters.",
      );
    return secret;
  }
  if (form.kind === "choice") {
    const values = data.getAll("choice").map(String);
    if (
      !values.length ||
      values.some((v) => !form.options.includes(v)) ||
      new Set(values).size !== values.length ||
      (!form.multiple && values.length !== 1)
    )
      throw Error(
        ja
          ? "選択肢の中から選んでください。"
          : "Select from the offered choices.",
      );
    return form.multiple ? values : values[0];
  }
  if (form.kind === "text") {
    const pairs = [];
    for (const field of form.fields) {
      pairs.push([
        field.name,
        await validateText(
          String(data.get(field.name) ?? ""),
          field.constraint,
          ja,
        ),
      ]);
    }
    return Object.fromEntries(pairs);
  }
  throw Error("Unknown input field.");
}
