/**
 * The downloadable-font store, wired to Electron.
 *
 * Twin of apps/slides/src/main/font-store.ts. The catalog and the store logic
 * live in `@genoffice/electron-utils/font-store`; this file is only the glue —
 * where the store dir is, which mirror this build ships, and how a finished
 * family gets back to the renderer.
 *
 * ## What "installed" means here, and why it is not slides' answer
 *
 * slides owns a main-process font registry and can answer "do I have this
 * family?" on its own. docs has no such registry: its picker learns what exists
 * from the renderer's Local Font Access API, and a family only becomes usable
 * by being registered as a FontFace. So the main process reports the half it
 * can actually know — "the files are in the store" — and the renderer folds the
 * system families in on top (see renderer/store-fonts.ts). Reporting every
 * catalog family as missing would offer a 28 MiB download of a font the reader
 * may well already have.
 *
 * ## The CDN reader is duplicated on purpose
 *
 * The three lines that pull `genofficeFontCdn` out of a package object and the
 * `app.isPackaged` branch below them are copied from the slides twin rather than
 * shared: they need `app`, and `electron-utils` is deliberately Electron-free.
 * A shared home for them would mean a new Electron-aware package layer, which is
 * a larger change than this one deserves. What they read is build metadata the
 * release pipeline writes, so both apps must agree on it anyway.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { app, net } from 'electron'
import {
  downloadFontFamily as downloadFromStore,
  familyDownloadedIn,
  listCatalog,
  normalizeCdnBaseUrl,
  type CatalogEntry,
  type FontStoreEnv,
} from '@genoffice/electron-utils/font-store'
import { FONT_CATALOG, type CatalogFile } from '@genoffice/electron-utils/font-catalog'

export type { CatalogEntry }

/** Where downloaded families live, beside the other per-user caches. */
export function fontStoreDir(): string {
  return join(app.getPath('userData'), 'fonts')
}

/** Read the build-injected font CDN URL from packaged app metadata. */
export function extractFontCdnBaseUrl(pkg: unknown): string | null {
  if (!pkg || typeof pkg !== 'object') return null
  const raw = (pkg as Record<string, unknown>).genofficeFontCdn
  if (!raw || typeof raw !== 'object') return null
  return normalizeCdnBaseUrl((raw as Record<string, unknown>).baseUrl)
}

/**
 * Official packages receive the URL through electron-builder extraMetadata.
 * Source/dev builds may opt in with an environment variable; without either,
 * all downloadable-font UI stays hidden — an affordance that cannot work is
 * worse than none, so `listDocsFontCatalog` returns [] and the picker shows
 * nothing at all.
 */
export function fontCdnBaseUrl(): string | null {
  if (!app.isPackaged) return normalizeCdnBaseUrl(process.env.GENOFFICE_FONT_CDN_URL)
  try {
    const pkg = JSON.parse(readFileSync(join(app.getAppPath(), 'package.json'), 'utf8')) as unknown
    return extractFontCdnBaseUrl(pkg)
  } catch {
    return null
  }
}

/** The store, described to the shared module in this app's terms. */
function storeEnv(): FontStoreEnv {
  const dir = fontStoreDir()
  return {
    dir,
    cdnBaseUrl: fontCdnBaseUrl(),
    fetchBytes: async (url) => {
      const response = await net.fetch(url)
      if (!response.ok) return { ok: false, status: response.status, bytes: new Uint8Array() }
      return {
        ok: true,
        status: response.status,
        bytes: new Uint8Array(await response.arrayBuffer()),
      }
    },
    // see the header: the renderer folds the system families in on top of this
    isFamilyAvailable: (family) => familyDownloadedIn(dir, family),
  }
}

/** Rows whose files are live on the mirror: the only ones the picker may offer. */
export function listDocsFontCatalog(): CatalogEntry[] {
  return listCatalog(storeEnv())
}

export function downloadDocsFontFamily(family: string): Promise<void> {
  return downloadFromStore(storeEnv(), family)
}

/** One stored cut of a family, as the renderer needs it to build a FontFace. */
export interface StoredFontFace {
  readonly style: CatalogFile['style']
  readonly bytes: Uint8Array
}

/**
 * Every catalog family whose cuts are all in the store.
 *
 * Deliberately independent of the mirror: the renderer re-registers these at
 * startup so a font downloaded in an earlier session is in the picker again.
 * Deriving that from the download catalog instead would make already-downloaded
 * fonts disappear whenever a build ships no mirror — the reader's own files
 * would stop working because of a setting they never touched.
 */
export function docsFontStoreFamilies(): string[] {
  const dir = fontStoreDir()
  return FONT_CATALOG.filter((f) => familyDownloadedIn(dir, f.family)).map((f) => f.family)
}

/**
 * The stored cuts of a family, or null when it is not fully in the store.
 *
 * All-or-nothing on purpose: a family half-fetched would register a regular
 * face and silently fall back for bold and italic, which reads as a font bug
 * rather than an incomplete download. The renderer registers these under the
 * catalog family name, so this is also the only place the store's files turn
 * into something a document can use.
 */
export function docsFontStoreFaces(family: string): StoredFontFace[] | null {
  const entry = FONT_CATALOG.find((f) => f.family === family)
  if (!entry) return null
  const dir = fontStoreDir()
  const faces: StoredFontFace[] = []
  for (const file of entry.files) {
    let bytes: Uint8Array
    try {
      bytes = new Uint8Array(readFileSync(join(dir, file.file)))
    } catch {
      return null
    }
    faces.push({ style: file.style, bytes })
  }
  return faces.length ? faces : null
}
