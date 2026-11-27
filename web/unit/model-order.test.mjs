import assert from "node:assert/strict";
import test from "node:test";
import { catalogSections } from "../src/model-order.ts";

// OpenRouter catalog observed on November 26, 2026.
const snapshot = [
  ["google/gemini-4-neon", 1795006977, "text"],
  ["openai/gpt-6.2-astra-pro", 1794938432, "text"],
  // An untiered OpenAI model must not raise the ChatGPT generation (#1154).
  ["openai/gpt-oss-120b", 1754000000, "text"],
  ["openai/gpt-6.2-astra", 1794938423, "text"],
  ["openai/gpt-6.2-sol-pro", 1794938409, "text"],
  ["openai/gpt-6.2-sol", 1794938371, "text"],
  ["google/gemini-4-hydrogen", 1794506460, "text"],
  ["google/gemini-4-carbon", 1794326871, "text"],
  ["anthropic/claude-fable-5.2", 1794253781, "text"],
  ["meta/muse-ember", 1794247648, "text"],
  ["openai/gpt-6.1-terra-pro", 1793038556, "text"],
  ["openai/gpt-6.1-terra", 1793038409, "text"],
  ["moonshotai/kimi-k3.1-pro", 1792778188, "text"],
  ["deepseek/deepseek-v4.1-flash-1017", 1792225624, "text"],
  ["meta/muse-spark-open", 1792087069, "text"],
  ["google/gemini-4-argon", 1791907477, "text"],
  ["meta/muse-wonder", 1791807945, "text"],
  ["anthropic/claude-haiku-5.5", 1791397883, "text"],
  ["openai/gpt-6.1-sol-pro", 1790702886, "text"],
  ["openai/gpt-6.1-sol", 1790702882, "text"],
  ["anthropic/claude-sonnet-5.5", 1790618686, "text"],
  ["openai/gpt-6-luna-pro", 1790100791, "text"],
  ["openai/gpt-6-luna", 1790100786, "text"],
  ["openai/gpt-6-sol-pro", 1790100781, "text"],
  ["openai/gpt-6-sol", 1790100775, "text"],
  ["anthropic/claude-opus-5.5", 1790094732, "text"],
  ["deepseek/deepseek-v4.1-flash", 1789021285, "text"],
  ["openai/gpt-6-astra", 1788552838, "text"],
  ["openai/gpt-6-astra-pro", 1788552835, "text"],
  ["meta/muse-spark-1.3-contributor", 1788381519, "text"],
  ["meta/muse-spark-1.3", 1788378359, "text"],
  ["anthropic/claude-fable-5.1", 1788285838, "text"],
  ["meta/muse-spark-1.2-contributor", 1787336476, "text"],
  ["deepseek/deepseek-v4-flash-vision-exp", 1787311563, "text"],
  ["deepseek/deepseek-v4-pro-0813", 1786549364, "text"],
  ["meta/muse-glimmer-30b", 1786302394, "text"],
  ["meta/muse-spark-1.2", 1785959287, "text"],
  ["deepseek/deepseek-v4-flash-0731", 1785478908, "text"],
  ["anthropic/claude-opus-5", 1784912544, "text"],
  ["moonshotai/kimi-k3", 1784215858, "text"],
  ["meta/muse-spark-1.1", 1784215741, "text"],
  ["anthropic/claude-sonnet-5", 1782843083, "text"],
  ["moonshotai/kimi-k2.7-code", 1781266361, "text"],
  ["anthropic/claude-fable-5", 1781007515, "text"],
  ["anthropic/claude-opus-4.8", 1779905091, "text"],
  ["deepseek/deepseek-v4-pro", 1777000679, "text"],
  ["deepseek/deepseek-v4-flash", 1777000666, "text"],
  ["moonshotai/kimi-k2.6", 1776699402, "text"],
  ["anthropic/claude-opus-4.7", 1776351100, "text"],
  ["anthropic/claude-sonnet-4.6", 1771342990, "text"],
  ["anthropic/claude-opus-4.6", 1770219050, "text"],
  ["moonshotai/kimi-k2.5", 1769487076, "text"],
  ["deepseek/deepseek-v3.2", 1764594642, "text"],
  ["anthropic/claude-opus-4.5", 1764010580, "text"],
  ["anthropic/claude-haiku-4.5", 1760547638, "text"],
  ["anthropic/claude-sonnet-4.5", 1759161676, "text"],
  ["deepseek/deepseek-v3.2-exp", 1759150481, "text"],
  ["deepseek/deepseek-v3.1-terminus", 1758548275, "text"],
  ["deepseek/deepseek-chat-v3.1", 1755779628, "text"],
  ["anthropic/claude-opus-4.1", 1754411591, "text"],
  ["deepseek/deepseek-r1-0528", 1748455170, "text"],
  ["deepseek/deepseek-chat-v3-0324", 1742824755, "text"],
  ["deepseek/deepseek-chat", 1735241320, "text"],
].map(([id, created, outputs]) => ({
  id,
  created,
  architecture: { output_modalities: outputs.split(",") },
}));

test("latest release for each shortcut tier", () => {
  assert.deepEqual(
    catalogSections(snapshot).shortcuts.map((m) => m.id),
    [
      "anthropic/claude-fable-5.2",
      "anthropic/claude-opus-5.5",
      "anthropic/claude-sonnet-5.5",
      "anthropic/claude-haiku-5.5",
      "openai/gpt-6.2-astra",
      "openai/gpt-6.2-sol",
      "openai/gpt-6.1-terra",
      "openai/gpt-6-luna",
      "google/gemini-4-neon",
      "google/gemini-4-carbon",
      "google/gemini-4-argon",
      "google/gemini-4-hydrogen",
      "deepseek/deepseek-v4.1-flash-1017",
      "deepseek/deepseek-v4-pro-0813",
      "moonshotai/kimi-k3.1-pro",
      "moonshotai/kimi-k3",
      "meta/muse-wonder",
      "meta/muse-spark-1.3",
    ],
  );
});

test("full catalog filters ineligible ids and keeps unlisted tiers", () => {
  const ids = [
    "z/model",
    "openai/gpt-6-unknown",
    "openai/gpt-5-sol",
    "openai/gpt-6-sol:batch",
    "a/model:free",
    "~alias",
    "v/demo-exp",
    "moonshotai/kimi-k2.7",
    "meta/muse-spark-open",
    "a/image",
    "openai/gpt-6-sol-pro",
    "a/model",
  ];
  const models = ids.map((id) => ({
    id,
    architecture: {
      output_modalities: id === "a/image" ? ["text", "image"] : ["text"],
    },
  }));
  assert.deepEqual(
    catalogSections(models).all.map((m) => m.id),
    [
      "openai/gpt-5-sol",
      "openai/gpt-6-unknown",
      "meta/muse-spark-open",
      "a/model",
      "z/model",
    ],
  );
});
