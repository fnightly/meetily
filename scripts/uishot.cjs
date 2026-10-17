// Playwright variant of cdpshot.cjs: same Tauri IPC stub, plus per-shot overrides.
// Usage: node uishot.cjs <url> <out.png> [overridesJs] [actionsJs]
const fs = require('fs');
const path = require('path');
const { chromium } = require('/vercel/sandbox/node_modules/playwright');

const src = fs.readFileSync(path.join(__dirname, 'cdpshot.cjs'), 'utf8');
const body = src.slice(src.indexOf('const TRANSCRIPTS'), src.indexOf('(async () =>'));
const STUB = new Function(body + '\nreturn STUB;')();

let [url, out, overrides = '', actions = ''] = process.argv.slice(2);
if (overrides.endsWith('.js')) overrides = fs.readFileSync(overrides, 'utf8');
if (actions.endsWith('.js')) actions = fs.readFileSync(actions, 'utf8');
const OVERRIDE = `
(() => {
  const base = window.__TAURI_INTERNALS__.invoke;
  const O = {}; ${overrides}
  window.__TAURI_INTERNALS__.invoke = async (cmd, args) => (O[cmd] ? O[cmd](args) : base(cmd, args));
  try { localStorage.setItem('meetily.locale', 'zh-CN'); } catch {}
})();`;

(async () => {
  const browser = await chromium.launch({ executablePath: '/usr/local/bin/chromium' });
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  page.on('pageerror', (e) => console.log('[pageerror]', e.message, (e.stack||'').split('\n').slice(0,3).join(' | ')));
  page.on('console', (m) => { if (m.type() === 'error') console.log('[console]', m.text().slice(0, 200)); });
  await page.addInitScript(STUB + OVERRIDE);
  await page.goto(url, { waitUntil: 'networkidle', timeout: 120000 });
  await page.waitForTimeout(2500);
  if (actions) await new Function('page', `return (async () => { ${actions} })()`)(page);
  await page.waitForTimeout(800);
  await page.screenshot({ path: out, fullPage: false });
  await browser.close();
  process.exit(0);
})().catch((e) => { console.error(e); process.exit(1); });
