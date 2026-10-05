import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { FONT_CATALOG } from '@genoffice/electron-utils/font-catalog'

/**
 * Main-process side of the downloadable-font store (src/main/docs-fonts.ts).
 *
 * The download discipline itself is the shared module's job and is tested
 * there; what is worth pinning down here is the two answers only this app can
 * give. The first is that a build shipping no mirror offers nothing, so the
 * picker shows no download section at all. The second is that "already
 * downloaded" is read from the store dir and not from the mirror: a font the
 * reader fetched in an earlier session has to keep working in a build that
 * offers no downloads, and deriving that from the catalog would take it away.
 */
const appState = { isPackaged: false, userData: '', appPath: '' }

vi.mock('electron', () => ({
  app: {
    get isPackaged() {
      return appState.isPackaged
    },
    getPath: (name: string) => (name === 'userData' ? appState.userData : ''),
    getAppPath: () => appState.appPath,
  },
  net: {
    fetch: async () => ({ ok: false, status: 599, arrayBuffer: async () => new ArrayBuffer(0) }),
  },
}))

const {
  docsFontStoreFaces,
  docsFontStoreFamilies,
  extractFontCdnBaseUrl,
  fontCdnBaseUrl,
  fontStoreDir,
  listDocsFontCatalog,
} = await import('../src/main/docs-fonts')

/** write a family's whole set of cuts into the store, as a download would */
function stageStore(family: string, byte = 1): void {
  const entry = FONT_CATALOG.find((f) => f.family === family)
  if (!entry) throw new Error(`not in catalog: ${family}`)
  // the store creates its own dir on download; the test stages the same end state
  mkdirSync(fontStoreDir(), { recursive: true })
  for (const file of entry.files) {
    writeFileSync(join(fontStoreDir(), file.file), new Uint8Array([byte]))
  }
}

describe('fontStoreDir', () => {
  beforeEach(() => {
    appState.userData = mkdtempSync(join(tmpdir(), 'docs-font-store-'))
  })

  it('keeps the store beside the app own per-user caches, under fonts/', () => {
    expect(fontStoreDir()).toBe(join(appState.userData, 'fonts'))
  })
})

describe('extractFontCdnBaseUrl', () => {
  it('reads the base URL out of packaged app metadata', () => {
    expect(extractFontCdnBaseUrl({ genofficeFontCdn: { baseUrl: 'https://cdn.test/v1' } })).toBe(
      'https://cdn.test/v1',
    )
  })

  it('drops a trailing slash so the store does not double it', () => {
    expect(extractFontCdnBaseUrl({ genofficeFontCdn: { baseUrl: 'https://cdn.test/v1/' } })).toBe(
      'https://cdn.test/v1',
    )
  })

  it.each([
    ['no metadata at all', null],
    ['metadata with no font section', { name: 'docs' }],
    ['a section that is not an object', { genofficeFontCdn: 'https://cdn.test' }],
    ['a base URL that is not a string', { genofficeFontCdn: { baseUrl: 7 } }],
  ])('refuses %s', (_label, pkg) => {
    expect(extractFontCdnBaseUrl(pkg)).toBeNull()
  })

  it('refuses a plain-HTTP mirror, which the shared store would refuse too', () => {
    expect(
      extractFontCdnBaseUrl({ genofficeFontCdn: { baseUrl: 'http://cdn.test/v1' } }),
    ).toBeNull()
  })
})

describe('fontCdnBaseUrl', () => {
  beforeEach(() => {
    appState.userData = mkdtempSync(join(tmpdir(), 'docs-font-store-'))
    appState.isPackaged = false
    delete process.env.GENOFFICE_FONT_CDN_URL
  })
  afterEach(() => {
    delete process.env.GENOFFICE_FONT_CDN_URL
  })

  it('takes the mirror from the environment in a source build', () => {
    process.env.GENOFFICE_FONT_CDN_URL = 'https://cdn.test/v1'
    expect(fontCdnBaseUrl()).toBe('https://cdn.test/v1')
  })

  it('is null in a source build with nothing configured, which hides the whole section', () => {
    expect(fontCdnBaseUrl()).toBeNull()
  })

  it('reads the packaged metadata rather than the environment once packaged', () => {
    appState.isPackaged = true
    process.env.GENOFFICE_FONT_CDN_URL = 'https://ignored.test/v1'
    const dir = mkdtempSync(join(tmpdir(), 'docs-font-pkg-'))
    appState.appPath = dir
    writeFileSync(
      join(dir, 'package.json'),
      JSON.stringify({ name: 'docs', genofficeFontCdn: { baseUrl: 'https://cdn.test/v2' } }),
    )
    expect(fontCdnBaseUrl()).toBe('https://cdn.test/v2')
  })

  it('is null when the packaged metadata cannot be read', () => {
    appState.isPackaged = true
    appState.appPath = join(tmpdir(), 'docs-font-no-such-app-dir')
    expect(fontCdnBaseUrl()).toBeNull()
  })
})

describe('the catalog and the store dir', () => {
  beforeEach(() => {
    appState.userData = mkdtempSync(join(tmpdir(), 'docs-font-store-'))
    appState.isPackaged = false
    delete process.env.GENOFFICE_FONT_CDN_URL
  })
  afterEach(() => {
    delete process.env.GENOFFICE_FONT_CDN_URL
  })

  it('offers nothing at all when the build ships no mirror', () => {
    // not an error state: the picker's download section is written to vanish
    expect(listDocsFontCatalog()).toEqual([])
  })

  it('offers the published families with a size, once a mirror is configured', () => {
    process.env.GENOFFICE_FONT_CDN_URL = 'https://cdn.test/v1'
    const rows = listDocsFontCatalog()
    const rubik = rows.find((r) => r.family === 'Rubik')
    expect(rubik).toMatchObject({ script: 'latin', license: 'OFL-1.1', installed: false })
    expect(rubik!.bytes).toBeGreaterThan(0)
    // unpublished families stay out so nobody can pick one that would 404
    expect(rows.some((r) => r.family === 'Noto Serif SC')).toBe(false)
  })

  it('reports a family already in the store as installed', () => {
    process.env.GENOFFICE_FONT_CDN_URL = 'https://cdn.test/v1'
    stageStore('Rubik')
    expect(listDocsFontCatalog().find((r) => r.family === 'Rubik')?.installed).toBe(true)
  })

  it('lists a half-fetched family as not installed rather than offering it as ready', () => {
    const entry = FONT_CATALOG.find((f) => f.family === 'Rubik')!
    const dir = fontStoreDir()
    mkdirSync(dir, { recursive: true })
    // only the regular cut: bold would silently fall back if this counted
    writeFileSync(
      join(dir, entry.files.find((f) => f.style === 'regular')!.file),
      new Uint8Array([1]),
    )
    expect(docsFontStoreFamilies()).not.toContain('Rubik')
    expect(docsFontStoreFaces('Rubik')).toBeNull()
  })

  it('hands back every cut of a stored family, in the catalog own styles', () => {
    stageStore('Lato', 9)
    const cuts = docsFontStoreFaces('Lato')
    expect(cuts!.map((c) => c.style).sort()).toEqual(
      FONT_CATALOG.find((f) => f.family === 'Lato')!
        .files.map((f) => f.style)
        .sort(),
    )
    expect(cuts![0].bytes.length).toBe(1)
  })

  it('has nothing to hand back for a family that was never fetched', () => {
    expect(docsFontStoreFaces('Lato')).toBeNull()
  })

  it('has nothing to hand back for a name that is not in the catalog', () => {
    expect(docsFontStoreFaces('Definitely Not A Font')).toBeNull()
  })

  it('lists the stored families whether or not a mirror is configured', () => {
    stageStore('Rubik')
    stageStore('Lato')
    // the point of reading the dir rather than the catalog: a reader's own
    // downloads must not disappear because this build offers no downloads
    expect(docsFontStoreFamilies().sort()).toEqual(['Lato', 'Rubik'])
  })

  it('leaves the store dir alone when listing families', () => {
    docsFontStoreFamilies()
    expect(existsSync(fontStoreDir())).toBe(false)
  })
})

describe('the bytes handed to the renderer', () => {
  beforeEach(() => {
    appState.userData = mkdtempSync(join(tmpdir(), 'docs-font-store-'))
  })

  it('are the file contents, not a name or a path', () => {
    stageStore('Lato', 9)
    const bytes = docsFontStoreFaces('Lato')![0].bytes
    expect(Array.from(bytes)).toEqual(
      Array.from(readFileSync(join(fontStoreDir(), 'Lato-regular.ttf'))),
    )
  })
})
