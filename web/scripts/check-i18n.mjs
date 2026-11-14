import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const catalogs = [
  "messages.ru.ts", "messages.ru.extra.ts", "messages.ru.dynamic.ts", "messages.ru.monitoring.ts",
];
const values = Object.create(null);
for (const file of catalogs) {
  const source = fs.readFileSync(path.join(root, "src/lib/i18n", file), "utf8");
  const matched = source.match(/=\s*(\{[\s\S]*\})\s*;/);
  if (!matched) throw new Error("Malformed localization catalog: " + file);
  const messages = JSON.parse(matched[1]);
  for (const [key, value] of Object.entries(messages)) {
    if (typeof value !== "string" || !value.trim()) {
      throw new Error("Empty translation " + file + ": " + key);
    }
    if (/\{\{[a-zA-Z]+\}\}/.test(key)) {
      const placeholders = [...key.matchAll(/\{\{([^}]+)\}\}/g)].map((match) => match[1]);
      for (const placeholder of placeholders) {
        if (!value.includes("{{" + placeholder + "}}")) {
          throw new Error("Missing substitution " + placeholder + " in " + key);
        }
      }
    }
    values[key] = value;
  }
}

const uiFiles = fs.readdirSync(path.join(root, "src/components"))
  .filter((file) => file.endsWith(".tsx"))
  .map((file) => path.join(root, "src/components", file));
uiFiles.push(path.join(root, "src/App.tsx"));

const missing = [];
const plain = [];
let references = 0;
for (const file of uiFiles) {
  const source = fs.readFileSync(file, "utf8");
  for (const match of source.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"\s*[,)]/g)) {
    const key = match[1];
    if (!key || key === "IndexLoom") continue;
    references++;
    if (!Object.hasOwn(values, key)) missing.push(path.relative(root, file) + ": " + key);
  }
  // Inline JS expression / names and author-provided tracker values are
  // intentionally excluded from this scanner.
  for (const match of source.matchAll(/>([^<>{}]+)</g)) {
    const phrase = match[1].replace(/\s+/g, " ").trim();
    if (!/[A-Za-z]{4}/.test(phrase) || /[;=]|=>|[()]/.test(phrase)
      || ["IndexLoom", "IndexLoom · v0.3", "Promise", "Jackett", "Prowlarr", "SOCKS4a", "SOCKS5h", "SMTP", "OIDC", "FlareSolverr", "HTTP"].includes(phrase) || phrase.startsWith("--")) continue;
    plain.push(path.relative(root, file) + ": " + phrase);
  }
}
if (missing.length || plain.length) {
  if (missing.length) console.error("Missing dictionary entries:\n" + missing.join("\n"));
  if (plain.length) console.error("Untranslated literal JSX text:\n" + plain.join("\n"));
  process.exit(1);
}
console.log("Localization audit passed:", Object.keys(values).length, "Russian entries,", references, "static UI references, no untranslated JSX literals.");
