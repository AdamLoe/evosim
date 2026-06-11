import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 800 } });
await page.goto('http://localhost:8743/notes/liquid-simulator.html');
await page.waitForLoadState('networkidle');

await page.evaluate(() => window.scrollTo(0, 1500));
await page.screenshot({ path: '/tmp/liquid-lists.png' });

await page.evaluate(() => window.scrollTo(0, 2800));
await page.screenshot({ path: '/tmp/liquid-code.png' });

await page.evaluate(() => window.scrollTo(0, 5800));
await page.screenshot({ path: '/tmp/liquid-blockquote.png' });

await browser.close();
console.log('done');
