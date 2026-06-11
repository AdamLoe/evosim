import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
await page.goto('http://localhost:8743/notes/liquid-simulator.html');
await page.waitForLoadState('networkidle');

// disable smooth scroll
await page.addStyleTag({ content: '* { scroll-behavior: auto !important; }' });

const shots = [
  [1100, 'h2-prose'],
  [1450, 'list-figure'],
  [2850, 'code-figure'],
  [4100, 'h3-headings'],
  [6820, 'blockquote'],
];

for (const [y, name] of shots) {
  await page.evaluate((scrollY) => window.scrollTo({ top: scrollY, behavior: 'instant' }), y);
  await page.waitForTimeout(100);
  await page.screenshot({ path: `/tmp/liquid-${name}.png` });
}

await browser.close();
console.log('done');
