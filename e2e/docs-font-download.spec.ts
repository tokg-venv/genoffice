import { test, expect } from '@playwright/test'
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import JSZip from 'jszip'
import { launchShell, closeAndSaveVideo, screenshotPath, waitForPageWithUrl } from './helpers'

/**
 * Downloadable fonts in the Font dialog (⌘D).
 *
 * The default case is the one that matters most: a build ships no font mirror
 * today, so the section must not be there at all — an affordance that cannot do
 * anything is worse than none. GENOFFICE_FONT_CDN_URL stands in for the mirror a
 * packaged build would read from its own metadata; the real network download
 * runs only with E2E_FONT_CDN=1.
 */

interface AidocsWindow {
  __aidocs?: { editor?: unknown; save?: () => Promise<unknown> }
}

async function minimalDocx(text: string): Promise<Buffer> {
  const zip = new JSZip()
  zip.file(
    '[Content_Types].xml',
    '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
  )
  zip.file(
    '_rels/.rels',
    '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
  )
  zip.file(
    'word/document.xml',
    `<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>${text}</w:t></w:r></w:p></w:body></w:document>`,
  )
  return zip.generateAsync({ type: 'nodebuffer' })
}

const catalogOf = (page: import('@playwright/test').Page) =>
  page.evaluate(async () => {
    const api = (
      window as unknown as {
        desktop?: {
          fontCatalog?: () => Promise<
            Array<{
              family: string
              script: string
              license: string
              installed: boolean
              bytes: number
            }>
          >
          fontStoreFamilies?: () => Promise<string[]>
        }
      }
    ).desktop
    return {
      catalog: (await api?.fontCatalog?.()) ?? [],
      stored: (await api?.fontStoreFamilies?.()) ?? [],
    }
  })

test.describe('docs downloadable fonts', () => {
  let dir: string
  let docPath: string

  test.beforeEach(async () => {
    dir = realpathSync(mkdtempSync(join(tmpdir(), 'genoffice-e2e-fontdl-')))
    docPath = join(dir, 'fonts.docx')
    writeFileSync(docPath, await minimalDocx('Spaced'))
  })

  test.afterEach(() => {
    rmSync(dir, { recursive: true, force: true })
  })

  const openDialog = async (app: Awaited<ReturnType<typeof launchShell>>['app']) => {
    const editorPage = await waitForPageWithUrl(app, '://docs/')
    await editorPage.waitForFunction(
      () => Boolean((window as unknown as AidocsWindow).__aidocs?.editor),
      undefined,
      { timeout: 30_000 },
    )
    await editorPage.locator('.doc-page').click()
    await editorPage.keyboard.press('ControlOrMeta+a')
    await editorPage.keyboard.press('ControlOrMeta+d')
    const dialog = editorPage.locator('.modal.font-dialog')
    await expect(dialog).toBeVisible()
    return { editorPage, dialog }
  }

  test('the download section follows the mirror configuration', async () => {
    test.setTimeout(180_000)
    const mirror = process.env.GENOFFICE_FONT_CDN_URL?.trim()
    const launched = await launchShell({
      onboardingSeen: true,
      videoDir: 'docs-font-download',
      openFile: docPath,
      env: mirror ? { GENOFFICE_FONT_CDN_URL: mirror } : {},
    })
    try {
      const { editorPage, dialog } = await openDialog(launched.app)
      const { catalog, stored } = await catalogOf(editorPage)

      if (!mirror) {
        // the shipped default: nothing to offer, so nothing offered
        expect(catalog).toEqual([])
        expect(stored).toEqual([])
        await expect(dialog.locator('.font-download')).toHaveCount(0)
        await dialog.screenshot({ path: screenshotPath('docs-font-download-none') })
        return
      }

      expect(catalog.length).toBeGreaterThan(0)
      // a build that ships no mirror must still report the reader's own files,
      // or a font downloaded earlier would vanish from the picker
      expect(stored.every((f) => catalog.some((c) => c.family === f))).toBe(true)

      const section = dialog.locator('.font-download')
      await expect(section).toBeVisible()
      await expect(section.locator('.font-download-head')).toHaveText('Downloadable fonts')
      // only the not-yet-usable families are offered: the main process reports
      // what is in the store and the renderer folds in the machine own, so a
      // family already present on this machine must not be offered a download
      const offerable = catalog.filter((c) => !c.installed)
      await expect(section.locator('.font-download-row')).toHaveCount(offerable.length)
      // the price is on the row: this is a tens-of-megabytes decision
      await expect(section.locator('.font-download-row').first()).toContainText(
        /\d+(\.\d+)? (B|KiB|MiB)/,
      )
      await expect(section.locator('.font-download-row').first()).toContainText('OFL-1.1')
      await section.screenshot({ path: screenshotPath('docs-font-download-catalog') })

      if (process.env.E2E_FONT_CDN !== '1') return

      // the real fetch: the row must become pickable, not merely disappear
      const rubik = section.locator('.font-download-row', { hasText: 'Rubik' })
      await rubik.click()
      await expect(rubik).toBeHidden({ timeout: 120_000 })
      const after = await catalogOf(editorPage)
      expect(after.catalog.find((c) => c.family === 'Rubik')?.installed).toBe(true)
      expect(after.stored).toContain('Rubik')
      // registered as a face, so the name is now offered in the picker
      await editorPage.getByLabel('Latin font').click()
      await expect(editorPage.getByRole('option', { name: 'Rubik' })).toBeVisible()
      await editorPage.keyboard.press('Escape')
      await dialog.getByRole('button', { name: 'OK' }).click()
      await expect(dialog).toBeHidden()
      const span = editorPage.locator('.doc-page span[data-doc-style]').first()
      await expect(span).toHaveCSS('font-family', /Rubik/)
    } finally {
      await closeAndSaveVideo(launched, 'docs-font-download')
    }
  })

  test('a restart keeps a downloaded family in the picker', async () => {
    test.setTimeout(240_000)
    if (process.env.E2E_FONT_CDN !== '1' || !process.env.GENOFFICE_FONT_CDN_URL?.trim()) {
      test.skip(true, 'needs a mirror and E2E_FONT_CDN=1 to download anything first')
    }
    const launched = await launchShell({
      onboardingSeen: true,
      videoDir: 'docs-font-download',
      openFile: docPath,
      env: { GENOFFICE_FONT_CDN_URL: process.env.GENOFFICE_FONT_CDN_URL },
    })
    try {
      const { dialog } = await openDialog(launched.app)
      const rubik = dialog.locator('.font-download-row', { hasText: 'Rubik' })
      await rubik.click()
      await expect(rubik).toBeHidden({ timeout: 120_000 })
    } finally {
      await closeAndSaveVideo(launched, 'docs-font-download')
    }

    // same scratch userData: nothing is re-downloaded, so a family that only
    // appeared because the download ran would be missing from this launch
    const relaunched = await launchShell({
      onboardingSeen: true,
      videoDir: 'docs-font-download',
      openFile: docPath,
      userDataDir: launched.userDataDir,
      env: { GENOFFICE_FONT_CDN_URL: process.env.GENOFFICE_FONT_CDN_URL },
    })
    try {
      const { editorPage, dialog } = await openDialog(relaunched.app)
      const { stored } = await catalogOf(editorPage)
      expect(stored).toContain('Rubik')
      await dialog.getByLabel('Latin font').click()
      await expect(editorPage.getByRole('option', { name: 'Rubik' })).toBeVisible()
    } finally {
      await closeAndSaveVideo(relaunched, 'docs-font-download')
    }
  })
})
