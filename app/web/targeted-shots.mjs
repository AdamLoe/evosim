import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
await page.goto('http://localhost:8743/notes/liquid-simulator.html');
await page.waitForLoadState('networkidle');

const shots = [
  [1100, 'h2-and-prose'],       // h2 + paragraph text
  [1450, 'list-figure'],         // ul/li figure
  [2830, 'code-figure'],         // pre/code figure
  [4100, 'h3-headings'],         // h3 subheadings
  [6800, 'blockquote'],          // blockquote callout
];

for (const [y, name] of shots) {
  await page.evaluate((scrollY) => window.scrollTo(0, scrollY), y);
  await page.screenshot({ path: `/tmp/liquid-${name}.png` });
}

await browser.close();
console.log('done');
