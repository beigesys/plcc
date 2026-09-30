// SPDX-License-Identifier: MPL-2.0
//
// Drives the app in headless Chromium: takes the screenshots in
// docs/screenshots/ and measures main-thread responsiveness with the
// simulator at a 1 ms period. Usage: npm run screenshots
// (needs `npx playwright install chromium` once).

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { chromium } from 'playwright'

const PORT = 5199
const URL = `http://localhost:${PORT}/`
const OUT = new globalThis.URL('../docs/screenshots/', import.meta.url).pathname
mkdirSync(OUT, { recursive: true })

// Screenshots run against the production build (`vite preview`): no dev-server
// dependency re-optimisation reloads in the middle of a run.
const vite = new globalThis.URL('../node_modules/.bin/vite', import.meta.url).pathname
const built = spawn(vite, ['build'], { stdio: 'inherit' })
await new Promise((resolve, reject) => built.on('exit', (c) => (c === 0 ? resolve() : reject(new Error(`build failed: ${c}`)))))
const server = spawn(vite, ['preview', '--port', String(PORT), '--strictPort'], { stdio: ['ignore', 'pipe', 'inherit'] })
server.stdout.on('error', () => {})
await new Promise((resolve, reject) => {
  server.stdout.on('data', (d) => String(d).includes('Local') && resolve())
  server.on('exit', (c) => reject(new Error(`vite exited ${c}`)))
  setTimeout(() => reject(new Error('vite did not start')), 30000)
})

const errors = []
const browser = await chromium.launch()
try {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'dark' })
  const page = await ctx.newPage()
  page.setDefaultTimeout(8000)
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))
  page.on('pageerror', (e) => errors.push(String(e)))
  const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
  const contact = (tag) => page.locator('g.ld-node', { has: page.locator('text.ld-tag', { hasText: new RegExp(`^${tag}$`) }) }).first()

  await page.goto(URL)
  await page.getByRole('application').waitFor()
  await page.waitForTimeout(400)

  // Offline: select the StartPB contact so the inspector shows its tag.
  await contact('StartPB').click()
  await page.waitForTimeout(150)
  await shot('offline')

  // Simulate: press StartPB (momentary), raise Level above 2000.
  await page.getByRole('radio', { name: 'Simulate' }).click()
  await page.waitForTimeout(500)
  await contact('StartPB').click()
  await page.keyboard.press('Space')
  await page.waitForTimeout(200)
  await page.keyboard.press('Space')
  const level = page.getByRole('slider', { name: 'I2 analog value' })
  await level.focus()
  await page.keyboard.press('End')
  await page.waitForTimeout(2200)
  await contact('Motor').first().click()
  await page.waitForTimeout(150)
  await shot('simulate')

  // Responsiveness of the UI thread: offline baseline, simulating at 10 ms, and at 1 ms.
  const measure = () =>
    page.evaluate(
      () =>
        new Promise((resolve) => {
          const gaps = []
          let long = 0
          let longest = 0
          const obs = new PerformanceObserver((list) => {
            for (const e of list.getEntries()) {
              long++
              longest = Math.max(longest, e.duration)
            }
          })
          try {
            obs.observe({ type: 'longtask', buffered: false })
          } catch {
            /* not supported */
          }
          let last = performance.now()
          const start = last
          const step = (t) => {
            gaps.push(t - last)
            last = t
            if (t - start < 3000) requestAnimationFrame(step)
            else {
              obs.disconnect()
              gaps.sort((a, b) => a - b)
              resolve({
                fps: +(gaps.length / 3).toFixed(1),
                p50FrameMs: +gaps[Math.floor(gaps.length / 2)].toFixed(1),
                p99FrameMs: +gaps[Math.floor(gaps.length * 0.99)].toFixed(1),
                worstFrameMs: +gaps[gaps.length - 1].toFixed(1),
                longTasks: long,
                longestTaskMs: +longest.toFixed(1),
              })
            }
          }
          requestAnimationFrame(step)
        }),
    )
  const setInterval_ = async (ms) => {
    await page.getByRole('button', { name: /^Tasks/ }).click()
    const interval = page.getByLabel('MainTask interval in milliseconds')
    await interval.fill(String(ms))
    await interval.press('Enter')
    await page.waitForTimeout(200)
    await page.getByRole('button', { name: /MainRoutine/ }).click()
    await page.waitForTimeout(1000)
  }
  const perf = {}
  await setInterval_(10)
  perf.simulate10ms = await measure()
  perf.simulate10ms.statusBar = (await page.locator('footer').innerText()).replace(/\s+/g, ' ')
  await setInterval_(1)
  perf.simulate1ms = await measure()
  perf.simulate1ms.statusBar = (await page.locator('footer').innerText()).replace(/\s+/g, ' ')
  await page.getByRole('radio', { name: 'Offline' }).click()
  await page.waitForTimeout(500)
  perf.offline = await measure()
  await page.getByRole('radio', { name: 'Simulate' }).click()
  await setInterval_(10)
  // Seal the motor in again after the restart.
  await contact('StartPB').click()
  await page.keyboard.press('Space')
  await page.waitForTimeout(200)
  await page.keyboard.press('Space')
  await page.getByRole('slider', { name: 'I2 analog value' }).focus()
  await page.keyboard.press('End')
  writeFileSync(`${OUT}perf.json`, JSON.stringify(perf, null, 2) + '\n')
  console.log('perf:', JSON.stringify(perf, null, 2))

  // I/O mapping while simulating (state dots live).
  await page.getByRole('button', { name: /^I\/O mapping/ }).click()
  await page.waitForTimeout(400)
  await shot('io-mapping')

  // Themes, on the routine in Simulate.
  await page.getByRole('button', { name: /MainRoutine/ }).click()
  for (const [name, file] of [
    ['Control Room', 'theme-control-room'],
    ['Blueprint', 'theme-blueprint'],
  ]) {
    await page.getByRole('button', { name: 'Theme' }).click()
    await page.getByRole('menuitem', { name: new RegExp(name) }).click()
    await page.waitForTimeout(400)
    await contact('Motor').first().click()
    await page.waitForTimeout(150)
    await shot(file)
  }
  await page.getByRole('button', { name: 'Theme' }).click()
  await page.getByRole('menuitem', { name: /Graphite/ }).click()

  // Online against the demo device.
  await page.getByRole('radio', { name: 'Online' }).click()
  await page.getByRole('button', { name: 'Demo device' }).click()
  await page.waitForTimeout(600)
  await contact('StartPB').click()
  await page.getByRole('button', { name: 'Force ON' }).click()
  await page.waitForTimeout(400)
  await page.getByRole('button', { name: 'Force OFF' }).click()
  await page.waitForTimeout(600)
  await shot('online-demo')

  // Command palette with quick entry.
  await page.getByRole('radio', { name: 'Offline' }).click()
  await page.keyboard.press('Control+k')
  await page.keyboard.type('XIC Start XIO Stop OTE Lamp')
  await page.waitForTimeout(200)
  await shot('palette')
  await page.keyboard.press('Enter')
  await page.waitForTimeout(300)

  // Operand editor with tag autocomplete.
  await contact('Start').first().dblclick()
  await page.keyboard.press('Control+a')
  await page.keyboard.type('Sta')
  await page.waitForTimeout(200)
  await shot('operand-editor')
  await page.keyboard.press('Escape')
  await page.keyboard.press('Escape')

  // Narrow window.
  await page.setViewportSize({ width: 1100, height: 760 })
  await page.waitForTimeout(300)
  await shot('narrow-1100')
} finally {
  await browser.close()
  server.kill()
}

if (errors.length) {
  console.error('console errors:\n' + errors.join('\n'))
  process.exitCode = 1
} else console.log('no console errors')
