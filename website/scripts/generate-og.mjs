// Renders website/og/template.html once per page, with that page's copy
// swapped in, and screenshots it at the standard 1200x630 OG size.
//
//   npm run generate:og
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import puppeteer from "puppeteer";

const ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const TEMPLATE = path.join(ROOT, "og", "template.html");

const PAGES = [
  {
    slug: "home",
    eyebrow: "Git client",
    title: "A native Git client that opens where you are",
    subtitle: "The commit graph, your worktrees, every diff. Fast, and in sync with the terminal.",
  },
  {
    slug: "releases",
    eyebrow: "Release notes",
    title: "What changed, and when",
    subtitle: "Every tagged release, built by CI and written up here.",
  },
  {
    slug: "brand",
    eyebrow: "Brand",
    title: "The Hinge",
    subtitle: "Two equal shapes, offset so they meet at one edge.",
  },
];

function render(template, page) {
  return template
    .replaceAll("{{EYEBROW}}", page.eyebrow)
    .replaceAll("{{TITLE}}", page.title)
    .replaceAll("{{SUBTITLE}}", page.subtitle);
}

const template = await readFile(TEMPLATE, "utf8");
const browser = await puppeteer.launch({ headless: "shell" });
const tab = await browser.newPage();
await tab.setViewport({ width: 1200, height: 630 });

for (const page of PAGES) {
  await tab.setContent(render(template, page), { waitUntil: "load" });
  const outPath = path.join(ROOT, "og", `${page.slug}.png`);
  await tab.screenshot({ path: outPath });
  console.log(`generated og/${page.slug}.png`);
}

await browser.close();
