#!/usr/bin/env node
// Captures a screenshot of the Tool Stats page (with chart).
// Usage: node scripts/capture-dashboard.js [port] [output]
// Defaults: port=8099, output=docs/images/dashboard.png

const { chromium } = require('playwright');
const path = require('path');

const port = process.argv[2] || '8099';
const output = process.argv[3] || path.join(__dirname, '..', 'docs', 'images', 'dashboard.png');

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  await page.setViewportSize({ width: 1400, height: 900 });

  console.log(`Navigating to http://localhost:${port} ...`);
  await page.goto(`http://localhost:${port}`, { waitUntil: 'networkidle' });

  // Navigate to Tool Stats page (has the calls-chart canvas)
  console.log('Clicking Tool Stats nav...');
  await page.click('[data-page="stats"]');

  // Wait for the chart canvas to be visible — chart data loads async
  console.log('Waiting for chart canvas...');
  await page.waitForSelector('#calls-chart', { state: 'visible', timeout: 10000 });

  // Small extra wait for Chart.js to finish rendering
  await page.waitForTimeout(800);

  console.log(`Saving screenshot to ${output} ...`);
  await page.screenshot({ path: output, fullPage: false });

  await browser.close();
  console.log('Done.');
})();
