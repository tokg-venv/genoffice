import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { DocsFontFace } from '../src/shared/ipc'
import {
  downloadAndRegister,
  registerStoreFamily,
  registerStoredFamilies,
  resetStoreFonts,
} from '../src/renderer/store-fonts'

const noteFontFacesChanged = vi.fn()
vi.mock('../src/renderer/line-metrics', () => ({
  noteFontFacesChanged: (families: readonly string[]) => noteFontFacesChanged(families),
}))

/**
 * What "downloaded" means in this app, tested at the only place it is decided.
 *
 * A file in the store dir changes nothing until it is a FontFace, so every case
 * here is about the gap between "the bytes arrived" and "the reader can use the
 * family" — a download that reported success over a family that never renders
 * is the bug this module exists to make impossible.
 */
class FakeFontFace {
  family: string
  source: ArrayBuffer
  descriptors: Record<string, string>
  /** family -> reject the next load for this family */
  static reject = new Set<string>()
  constructor(family: string, source: ArrayBuffer, descriptors: Record<string, string>) {
    this.family = family
    this.source = source
    this.descriptors = descriptors
  }
  load(): Promise<this> {
    return FakeFontFace.reject.has(this.family)
      ? Promise.reject(new Error('not a font'))
      : Promise.resolve(this)
  }
}

let faces: Set<FakeFontFace>
let events: string[]

/** a cut with distinct bytes, so a mixed-up buffer is visible in an assertion */
const cut = (style: DocsFontFace['style'], byte: number): DocsFontFace => ({
  style,
  bytes: new Uint8Array([byte, byte, byte, byte]),
})

const api = (overrides: Partial<Window['desktop']> = {}): void => {
  Object.defineProperty(window, 'desktop', {
    configurable: true,
    value: {
      fontStoreFaces: async () => [cut('regular', 1), cut('bold', 2)],
      fontStoreFamilies: async () => ['Rubik'],
      fontDownload: async () => ({ ok: true }),
      ...overrides,
    },
  })
}

describe('registerStoreFamily', () => {
  beforeEach(() => {
    faces = new Set<FakeFontFace>()
    events = []
    FakeFontFace.reject = new Set()
    resetStoreFonts()
    noteFontFacesChanged.mockClear()
    vi.stubGlobal('FontFace', FakeFontFace)
    Object.defineProperty(document, 'fonts', {
      configurable: true,
      value: {
        add: (f: FakeFontFace) => {
          faces.add(f)
          events.push(`add:${f.family}:${f.descriptors.weight}/${f.descriptors.style}`)
        },
        delete: (f: FakeFontFace) => {
          faces.delete(f)
          events.push(`del:${f.family}`)
        },
        dispatchEvent: () => {
          events.push('loadingdone')
          return true
        },
      },
    })
    api()
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    resetStoreFonts()
  })

  it('registers every cut under the family name with its own weight and style', async () => {
    await expect(
      registerStoreFamily('Rubik', [cut('regular', 1), cut('bold', 2), cut('italic', 3)]),
    ).resolves.toBe(true)
    expect(events).toEqual([
      'add:Rubik:400/normal',
      'add:Rubik:700/normal',
      'add:Rubik:400/italic',
      'loadingdone',
    ])
  })

  it('marks bold italic as both, which is the cut a two-field mapping loses', async () => {
    await registerStoreFamily('Rubik', [cut('boldItalic', 4)])
    expect(events).toContain('add:Rubik:700/italic')
  })

  it('registers nothing when one cut fails to parse', async () => {
    FakeFontFace.reject.add('Rubik')
    await expect(registerStoreFamily('Rubik', [cut('regular', 1)])).resolves.toBe(false)
    expect(faces.size).toBe(0)
    expect(events).toEqual([])
  })

  it('registers nothing for an empty cut list', async () => {
    await expect(registerStoreFamily('Rubik', [])).resolves.toBe(false)
    expect(faces.size).toBe(0)
  })

  it('replaces the faces of a re-download instead of stacking a second set', async () => {
    await registerStoreFamily('Rubik', [cut('regular', 1), cut('bold', 2)])
    await registerStoreFamily('Rubik', [cut('regular', 9)])
    expect(faces.size).toBe(1)
    expect(events.filter((e) => e.startsWith('del:'))).toEqual(['del:Rubik', 'del:Rubik'])
  })

  it('copies a cut that is a view onto a larger buffer', async () => {
    // an IPC receive or a pooled buffer arrives this way: handing FontFace the
    // whole backing store would register whatever else lives in it
    const backing = new Uint8Array([0, 0, 0, 0, 7, 7, 7, 7])
    const view = backing.subarray(4)
    await registerStoreFamily('Rubik', [{ style: 'regular', bytes: view }])
    const registered = [...faces][0]
    expect(registered.source.byteLength).toBe(4)
    expect([...new Uint8Array(registered.source)]).toEqual([7, 7, 7, 7])
  })

  it('tells the metrics caches a family changed', async () => {
    await registerStoreFamily('Rubik', [cut('regular', 1)])
    expect(noteFontFacesChanged).toHaveBeenCalledWith(['Rubik'])
  })

  it('fails closed with no document rather than throwing on the way to document.fonts', async () => {
    // the faces are built and loaded fine; the first touch of `document` is the
    // add below, which sits outside the per-cut try — so without the guard this
    // rejects instead of resolving false
    vi.stubGlobal('document', undefined)
    await expect(registerStoreFamily('Rubik', [cut('regular', 1)])).resolves.toBe(false)
  })

  it('fails closed with no FontFace to build from', async () => {
    vi.stubGlobal('FontFace', undefined)
    await expect(registerStoreFamily('Rubik', [cut('regular', 1)])).resolves.toBe(false)
  })
})

describe('registerStoredFamilies', () => {
  beforeEach(() => {
    faces = new Set<FakeFontFace>()
    FakeFontFace.reject = new Set()
    resetStoreFonts()
    vi.stubGlobal('FontFace', FakeFontFace)
    Object.defineProperty(document, 'fonts', {
      configurable: true,
      value: {
        add: (f: FakeFontFace) => faces.add(f),
        delete: () => {},
        dispatchEvent: () => true,
      },
    })
    // every block starts from the same bridge: a test that inherits the previous
    // one's stub can pass for the wrong reason
    api()
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    resetStoreFonts()
  })

  it('re-registers what the store already holds, so a restart keeps the font', async () => {
    api({ fontStoreFamilies: async () => ['Rubik', 'Lato'] })
    await expect(registerStoredFamilies().then((f) => [...f].sort())).resolves.toEqual([
      'Lato',
      'Rubik',
    ])
    expect([...new Set([...faces].map((f) => f.family))].sort()).toEqual(['Lato', 'Rubik'])
  })

  it('skips a family whose bytes no longer parse, and keeps the rest', async () => {
    api({ fontStoreFamilies: async () => ['Rubik', 'Lato'] })
    FakeFontFace.reject.add('Rubik')
    await expect(registerStoredFamilies()).resolves.toEqual(['Lato'])
  })

  it('returns nothing when the main process cannot be reached', async () => {
    api({
      fontStoreFamilies: async () => {
        throw new Error('no bridge')
      },
    })
    await expect(registerStoredFamilies()).resolves.toEqual([])
  })
})

describe('downloadAndRegister', () => {
  beforeEach(() => {
    faces = new Set<FakeFontFace>()
    FakeFontFace.reject = new Set()
    resetStoreFonts()
    vi.stubGlobal('FontFace', FakeFontFace)
    Object.defineProperty(document, 'fonts', {
      configurable: true,
      value: {
        add: (f: FakeFontFace) => faces.add(f),
        delete: () => {},
        dispatchEvent: () => true,
      },
    })
    api()
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    resetStoreFonts()
  })

  it('reports failure when the download itself failed', async () => {
    api({ fontDownload: async () => ({ ok: false, error: 'checksum mismatch' }) })
    await expect(downloadAndRegister('Rubik')).resolves.toEqual({
      ok: false,
      error: 'checksum mismatch',
    })
  })

  it('reports failure when the stored bytes never arrive, even though the download said ok', async () => {
    // the exact gap this module closes: a download can land in the store and
    // still leave the reader with a name that renders in a substitute face
    api({ fontStoreFaces: async () => null })
    await expect(downloadAndRegister('Rubik')).resolves.toEqual({
      ok: false,
      error: 'font unusable',
    })
    expect(faces.size).toBe(0)
  })

  it('reports failure when the faces will not parse', async () => {
    FakeFontFace.reject.add('Rubik')
    await expect(downloadAndRegister('Rubik')).resolves.toEqual({
      ok: false,
      error: 'font unusable',
    })
  })

  it('registers the family when the download and the bytes both land', async () => {
    await expect(downloadAndRegister('Rubik')).resolves.toEqual({ ok: true })
    expect([...faces].map((f) => f.family)).toEqual(['Rubik', 'Rubik'])
  })
})
