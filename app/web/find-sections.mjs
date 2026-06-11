import { chromium } from '@playwright/test';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 800 } });
await page.goto('http://localhost:8743/notes/liquid-simulator.html');
await page.waitForLoadState('networkidle');

const positions = await page.evaluate(() => {
  const elements = [...document.querySelectorAll('h2, h3, figure, blockquote')];
  return elements.map(el => {
    const rect = el.getBoundingClientRect();
    return { tag: el.tagName, text: el.textContent.trim().slice(0, 40), top: Math.round(rect.top + window.scrollY) };
  });
});
console.log(JSON.stringify(positions, null, 2));
await browser.close();
