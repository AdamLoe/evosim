import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 800 } });
await page.goto('http://localhost:8743/notes/liquid-simulator.html');
await page.waitForLoadState('networkidle');

const totalHeight = await page.evaluate(() => document.body.scrollHeight);
console.log('total scroll height:', totalHeight);

// screenshot right after the hero figure (near the text content start)
for (const y of [totalHeight * 0.15, totalHeight * 0.3, totalHeight * 0.5, totalHeight * 0.75]) {
  await page.evaluate((scrollY) => window.scrollTo(0, scrollY), Math.round(y));
  await page.screenshot({ path: `/tmp/liquid-y${Math.round(y)}.png` });
  console.log('shot at', Math.round(y));
}

await browser.close();
