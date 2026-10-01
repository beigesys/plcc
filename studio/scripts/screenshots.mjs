// SPDX-License-Identifier: MPL-2.0
//
// Drives the production build in headless Chromium, checks what the studio
// does end to end, and writes docs/screenshots/*.png and perf.json:
//
//   - the demo project, Simulate with plcc's REAL compiler (downloaded into
//     the page, compiled to wasm32, linked, run by @plcc/plc-wasm): StartPB
//     seals Motor in and RunTimer accumulates;
//   - right-click a contact → Change type → XIO;
//   - plcc's diagnostics on a broken rung, and the Problems panel;
//   - the ST view;
//   - importing an L5X fixture and an ST file;
//   - the Download dialog building the program image;
//   - Online against the demo device, with the serial log.
//
// Any failed check or console error exits non-zero.
//
//   npm run screenshots                    # base /
//   STUDIO_BASE=/plcc/ npm run screenshots # as GitHub Pages serves it
//
// Needs `npx playwright install chromium` once, and the compiler built
// (packages/plcc-compiler-wasm: sh build.sh) for the Simulate checks.

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { chromium } from 'playwright'

const PORT = 5199
const BASE = process.env.STUDIO_BASE ?? '/'
const URL = `http://localhost:${PORT}${BASE}`
const OUT = new globalThis.URL('../docs/screenshots/', import.meta.url).pathname
const FIXTURES = new globalThis.URL('../../tests/fixtures/', import.meta.url).pathname
mkdirSync(OUT, { recursive: true })

const vite = new globalThis.URL('../node_modules/.bin/vite', import.meta.url).pathname
const built = spawn(vite, ['build'], { stdio: 'inherit', env: { ...process.env, STUDIO_BASE: BASE } })
await new Promise((resolve, reject) => built.on('exit', (c) => (c === 0 ? resolve() : reject(new Error(`build failed: ${c}`)))))
const server = spawn(vite, ['preview', '--port', String(PORT), '--strictPort'], {
  stdio: ['ignore', 'pipe', 'inherit'],
  env: { ...process.env, STUDIO_BASE: BASE },
})
server.stdout.on('error', () => {})
await new Promise((resolve, reject) => {
  server.stdout.on('data', (d) => String(d).includes('Local') && resolve())
  server.on('exit', (c) => reject(new Error(`vite exited ${c}`)))
  setTimeout(() => reject(new Error('vite did not start')), 30000)
})

const errors = []
const results = []
function check(name, ok, detail = '') {
  results.push({ name, ok, detail })
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ` — ${detail}` : ''}`)
  if (!ok) process.exitCode = 1
}

const browser = await chromium.launch()
const perf = {}
try {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: 'dark' })
  const page = await ctx.newPage()
  page.setDefaultTimeout(15000)
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))
  page.on('pageerror', (e) => errors.push(String(e)))
  const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
  const contact = (tag) => page.locator('g.ld-node', { has: page.locator('text.ld-tag', { hasText: new RegExp(`^${tag}$`) }) }).first()
  const box = (instr) => page.locator('g.ld-node', { has: page.locator('text.ld-instr', { hasText: new RegExp(`^${instr}$`) }) }).first()
  const rungText = (i) => page.locator('[data-rung-text]').nth(i).inputValue()

  await page.goto(URL)
  await page.getByRole('application').waitFor()
  await page.waitForTimeout(800)
  check('opens with the demo project', (await page.locator('[data-rung-text]').count()) === 3)
  // plcc's front end loads in a worker and checks the project.
  await page.waitForFunction(() => {
    const b = [...document.querySelectorAll('button')].find((x) => x.getAttribute('aria-label')?.startsWith('Problems:'))
    return b && !b.textContent?.includes('checking') && /Problems: \d+ errors/.test(b.getAttribute('aria-label') ?? '') && performance.now() > 1500
  })
  const plabel = (await page.getByRole('button', { name: /^Problems:/ }).getAttribute('aria-label')) ?? ''
  check('plcc check: the demo has no problems', /0 errors, 0 warnings/.test(plabel), plabel)

  // Offline: select the StartPB contact so the inspector shows its tag.
  await contact('StartPB').click()
  await page.waitForTimeout(150)
  await shot('offline')

  // ---- right-click → Change type → XIO
  await contact('StartPB').click({ button: 'right' })
  const menu = page.getByRole('menu')
  await menu.waitFor()
  check('right-click opens the element menu', await menu.getByRole('menuitem', { name: /Edit operand/ }).isVisible())
  await menu.getByRole('menuitem', { name: 'Change type' }).hover()
  await page.waitForTimeout(150)
  await shot('context-menu')
  await page.getByRole('menuitem', { name: /^XIO$/ }).click()
  await page.waitForTimeout(200)
  check('Change type → XIO', (await rungText(0)).startsWith('[XIO(StartPB) ,'), await rungText(0))
  await page.locator('#rung-list').focus()
  await page.keyboard.press('Control+z')
  await page.waitForTimeout(200)
  // Keyboard: Shift+F10 opens the same menu at the selection.
  await contact('StopPB').click()
  await page.keyboard.press('Shift+F10')
  await menu.waitFor()
  check('Shift+F10 opens the menu', await menu.getByRole('menuitem', { name: /Edit operand/ }).isVisible())
  await page.keyboard.press('Escape')

  // ---- diagnostics on a broken rung
  const rt = page.locator('[data-rung-text]').nth(1)
  await rt.click()
  await rt.fill('XIC(Motor)TON(Motor,5000,0)OTE(Nowhere.DN);')
  await rt.press('Enter')
  await page.locator('[data-mark=error]').first().waitFor()
  const marks = await page.locator('[data-mark=error]').count()
  check('a broken rung gets plcc diagnostics on its elements', marks >= 2, `${marks} elements marked`)
  await page.getByRole('button', { name: /^Problems:/ }).click()
  await page.getByRole('region', { name: 'Problems' }).waitFor()
  const probs = await page.getByRole('region', { name: 'Problems' }).innerText()
  check('the Problems panel lists them', /TIMER/.test(probs) && /Nowhere/.test(probs), probs.split('\n').slice(2, 4).join(' / '))
  await shot('diagnostics')
  await page.getByRole('button', { name: 'Close problems' }).click()
  await page.locator('#rung-list').focus()
  await page.keyboard.press('Control+z')
  await page.waitForTimeout(800)

  // ---- ST view
  await page.getByRole('button', { name: 'ST view' }).click()
  const stText = page.getByTestId('st-view-text')
  await page.waitForFunction(() => /METHOD R_MainRoutine/.test(document.querySelector('[data-testid=st-view-text]')?.textContent ?? ''))
  check('the ST view shows the generated ST', /lx__ton|RunTimer/.test(await stText.innerText()))
  await contact('Motor').click()
  await shot('st-view')
  await page.getByRole('radio', { name: 'IEC translation' }).click()
  await page.waitForFunction(() => /RunTimer\(IN :=/.test(document.querySelector('[data-testid=st-view-text]')?.textContent ?? ''))
  check('… and the IEC translation', true)
  await page.getByRole('button', { name: 'Close ST view' }).click()

  // ---- Simulate with plcc's real compiler
  const t0 = Date.now()
  await page.getByRole('radio', { name: 'Simulate' }).click()
  const status = page.getByTestId('sim-status')
  const downloads = []
  for (let i = 0; i < 240; i++) {
    const text = await status.innerText().catch(() => '')
    if (/downloading/.test(text)) downloads.push(text)
    if ((await status.getAttribute('data-engine').catch(() => '')) === 'plcc') break
    await page.waitForTimeout(250)
  }
  const engineMs = Date.now() - t0
  check('Simulate runs plcc\'s own wasm32 build', (await status.getAttribute('data-engine')) === 'plcc', `${engineMs} ms to the first build`)
  perf.compilerFirstLoadMs = engineMs
  await contact('StartPB').click()
  await page.keyboard.press('Space')
  await page.waitForTimeout(300)
  await page.keyboard.press('Space')
  await page.waitForTimeout(600)
  await contact('Motor').click()
  const insp = await page.getByRole('region', { name: 'Selected tag' }).innerText()
  check('StartPB seals Motor in', /Value\s*1/.test(insp))
  const acc = async () => Number(/Accum\s*(\d+)/.exec((await box('TON').textContent()) ?? '')?.[1] ?? NaN)
  const a1 = await acc()
  await page.waitForTimeout(1000)
  const a2 = await acc()
  check('RunTimer accumulates', a2 > a1 && a2 - a1 >= 700 && a2 - a1 <= 1500, `ACC ${a1} → ${a2}`)
  const level = page.getByRole('slider', { name: 'I2 analog value' })
  await level.focus()
  await page.keyboard.press('End')
  await page.waitForTimeout(500)
  await contact('Motor').click()
  await shot('simulate')

  // Responsiveness of the UI thread with plcc's program running.
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
                longTasks: long,
                longestTaskMs: +longest.toFixed(1),
              })
            }
          }
          requestAnimationFrame(step)
        }),
    )
  perf.simulatePlcc = await measure()
  perf.simulatePlcc.statusBar = (await page.locator('footer').innerText()).replace(/\s+/g, ' ')

  // A reload: the compiler comes from Cache Storage.
  await page.getByRole('radio', { name: 'Offline' }).click()
  await page.reload()
  await page.getByRole('application').waitFor()
  const t1 = Date.now()
  await page.getByRole('radio', { name: 'Simulate' }).click()
  for (let i = 0; i < 240; i++) {
    if ((await page.getByTestId('sim-status').getAttribute('data-engine').catch(() => '')) === 'plcc') break
    await page.waitForTimeout(100)
  }
  perf.compilerCachedLoadMs = Date.now() - t1
  check('the compiler is cached across reloads', perf.compilerCachedLoadMs < perf.compilerFirstLoadMs + 2000, `${perf.compilerCachedLoadMs} ms`)

  // I/O mapping while simulating (state dots live).
  await page.getByRole('button', { name: /^I\/O mapping/ }).click()
  await page.waitForTimeout(400)
  await shot('io-mapping')
  await page.getByRole('button', { name: /MainRoutine/ }).click()

  // Themes.
  for (const [name, file] of [
    ['Control Room', 'theme-control-room'],
    ['Blueprint', 'theme-blueprint'],
  ]) {
    await page.getByRole('button', { name: 'Theme' }).click()
    await page.getByRole('menuitem', { name: new RegExp(name) }).click()
    await page.waitForTimeout(400)
    await contact('Motor').click()
    await page.waitForTimeout(150)
    await shot(file)
  }
  await page.getByRole('button', { name: 'Theme' }).click()
  await page.getByRole('menuitem', { name: /Graphite/ }).click()
  await page.getByRole('radio', { name: 'Offline' }).click()

  // ---- Download: compile for the Opta and link the program image (no hardware)
  await page.getByRole('button', { name: 'Download' }).click()
  await page.locator('[data-step=image][data-state=ok], [data-step][data-state=failed]').first().waitFor({ timeout: 60000 })
  const steps = await page.getByTestId('download-steps').innerText()
  check('Download builds the program image in the browser', /bytes at 0x8180000/.test(steps), steps.split('\n').slice(0, 4).join(' / '))
  await shot('download')
  await page.getByRole('button', { name: 'Cancel' }).click()

  // ---- import an L5X fixture and an ST file
  const importFile = async (path, name) => {
    await page.getByRole('button', { name: 'File' }).click()
    await page.getByRole('menuitem', { name: /^Import/ }).click()
    await page.getByLabel('Import file').setInputFiles(path)
    await page.getByTestId('import-result').waitFor()
    const text = await page.getByTestId('import-result').innerText()
    await shot(name)
    await page.getByRole('button', { name: 'Add to this project' }).click()
    await page.waitForTimeout(400)
    return text
  }
  const l5x = await importFile(`${FIXTURES}l5x/timers_counters.L5X`, 'import-l5x')
  check('imports an L5X file', /program\(s\)/.test(l5x) && (await page.locator('[data-rung-text]').count()) > 0, l5x.split('\n')[0])
  const st = await importFile(`${FIXTURES}programs/motor_control.st`, 'import-st')
  check('imports an ST file', /program\(s\)/.test(st), st.split('\n')[0])
  await page.waitForTimeout(800)

  // ---- Online against the demo device, with the serial log
  await page.getByRole('button', { name: /^MainRoutine/ }).first().click()
  await page.getByRole('radio', { name: 'Online' }).click()
  await page.getByRole('button', { name: 'Demo device' }).click()
  await page.waitForTimeout(800)
  await page.getByRole('button', { name: 'Serial log' }).click()
  await contact('StartPB').click()
  await page.getByRole('button', { name: 'Force ON' }).click()
  await page.waitForTimeout(400)
  await page.getByRole('button', { name: 'Force OFF' }).click()
  await page.waitForTimeout(800)
  const log = await page.getByTestId('serial-log').innerText()
  check('Online: the serial log shows the console traffic', /> info/.test(log) && /< \{"device":"arduino-opta"/.test(log) && /> mw /.test(log))
  check('Online: the program state from info', /program running/.test(await page.getByTestId('program-state').innerText()))
  await shot('online-demo')
  await page.getByRole('button', { name: 'Disconnect' }).click()

  // Command palette with quick entry.
  await page.getByRole('radio', { name: 'Offline' }).click()
  await page.keyboard.press('Control+k')
  await page.keyboard.type('XIC Start XIO Stop OTE Lamp')
  await page.waitForTimeout(200)
  await shot('palette')
  await page.keyboard.press('Escape')

  // Operand editor with tag autocomplete.
  await contact('StopPB').dblclick()
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
} catch (e) {
  check('the run completed', false, e instanceof Error ? e.message.split('\n')[0] : String(e))
} finally {
  await browser.close()
  server.kill()
}

writeFileSync(`${OUT}perf.json`, `${JSON.stringify(perf, null, 2)}\n`)
console.log('perf:', JSON.stringify(perf))
const benign = (e) => /Failed to load resource.*(favicon)/.test(e)
const real = errors.filter((e) => !benign(e))
if (real.length) {
  console.error(`console errors:\n${real.join('\n')}`)
  process.exitCode = 1
}
console.log(`${results.filter((r) => r.ok).length}/${results.length} checks passed${real.length ? `, ${real.length} console errors` : ''}`)
