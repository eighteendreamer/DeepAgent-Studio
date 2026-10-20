const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const { createInstance } = require("i18next");

const sourceRoot = path.resolve(__dirname, "../src");
const localeRoot = path.join(sourceRoot, "locales");
const resources = Object.fromEntries(fs.readdirSync(localeRoot)
  .filter(file => file.endsWith(".json"))
  .map(file => [file.slice(0, -5), {
    translation: JSON.parse(fs.readFileSync(path.join(localeRoot, file), "utf8")),
  }]));
const keys = [
  "limit", "recognizing", "processing", "failed", "readFailed", "text", "file",
  "removeImage", "removeAttachment", "previewImage", "previewNamedImage", "closePreview",
  "statusLabel",
];

async function translator() {
  const i18n = createInstance();
  await i18n.init({ resources, lng: "zh", fallbackLng: "zh", interpolation: { escapeValue: false } });
  return i18n;
}

test("attachment limits follow the current UI language and interpolate the real limit", async () => {
  const i18n = await translator();
  assert.equal(i18n.t("composer.attachments.limit", { count: 5 }), "最多可添加 5 个附件");
  await i18n.changeLanguage("en");
  assert.equal(i18n.t("composer.attachments.limit", { count: 5 }), "Maximum 5 attachments allowed");
  await i18n.changeLanguage("zh-TW");
  assert.equal(i18n.t("composer.attachments.limit", { count: 7 }), "最多可新增 7 個附件");
});

test("every shipped language has attachment strings with matching interpolation variables", async () => {
  const i18n = await translator();
  const placeholders = value => [...value.matchAll(/{{\s*(\w+)\s*}}/g)].map(match => match[1]).sort();
  for (const [language, resource] of Object.entries(resources)) {
    const entries = resource.translation.composer?.attachments;
    for (const key of keys) {
      assert.equal(typeof entries?.[key], "string", `${language}: missing attachment string ${key}`);
      assert.ok(entries[key].trim(), `${language}: empty attachment string ${key}`);
      assert.deepEqual(placeholders(entries[key]), placeholders(resources.zh.translation.composer.attachments[key]), `${language}: ${key} placeholders`);
    }
    const filename = "截图<&>.png";
    const label = i18n.t("composer.attachments.previewNamedImage", { lng: language, name: filename });
    assert.ok(label.includes(filename), `${language}: preserve the original filename`);
    assert.doesNotMatch(label, /{{|composer\.attachments/);
  }
});

test("the warning and disabled attachment control use the same translated limit", () => {
  const composer = fs.readFileSync(path.join(sourceRoot, "components/Composer.tsx"), "utf8");
  assert.match(composer, /attachmentLimitMessage\s*=\s*t\("composer\.attachments\.limit",\s*{\s*count:\s*MAX_COMPOSER_ATTACHMENTS,?\s*}\)/);
  assert.match(composer, /message\.warning\(attachmentLimitMessage\)/);
  assert.match(composer, /disabledTitle={attachmentLimitMessage}/);
  assert.doesNotMatch(composer, /Maximum \$\{MAX_COMPOSER_ATTACHMENTS\} attachments allowed/);
});
