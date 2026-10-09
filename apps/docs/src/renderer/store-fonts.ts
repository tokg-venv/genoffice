import { useSyncExternalStore } from 'react'
import type { DocsFontFace } from '../shared/ipc'
import { sfntBuffer } from './embedded-fonts'
import { noteFontFacesChanged } from './line-metrics'

/**
 * Families the reader has downloaded, as this app can actually use them.
 *
 * ## Why the renderer is the one that decides
 *
 * A downloaded family does nothing on its own. It is a file in the store dir
 * that the renderer must turn into a FontFace before a single glyph changes,
 * and only then does the name belong in the picker. So "installed" in this app
 * means *registered and rendering*, not *present on disk* — the main process
 * reports the disk half (main/docs-fonts.ts) and this module owns the rest.
 * A family whose bytes never arrive, or whose faces fail to parse, stays
 * unregistered and therefore keeps being offered: the row is the truth, not the
 * download's exit code.
 *
 * ## Why registration is not a one-off
 *
 * Faces live in `document.fonts`, which dies with the window. A family
 * downloaded yesterday is on disk today and would silently vanish from the
 * picker after a restart unless something re-registers it, so
 * `registerStoredFamilies` runs at startup and is the same code path a download
 * takes. One path, so a family cannot be reachable one way and not the other.
 */
const STYLE_FACE: Record<DocsFontFace['style'], { weight: string; style: string }> = {
  regular: { weight: '400', style: 'normal' },
  bold: { weight: '700', style: 'normal' },
  italic: { weight: '400', style: 'italic' },
  boldItalic: { weight: '700', style: 'italic' },
}

const faces = new Map<string, readonly FontFace[]>()
let snapshot: readonly string[] = []
const listeners = new Set<() => void>()

function publish(): void {
  snapshot = [...faces.keys()].sort((a, b) => a.localeCompare(b))
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Families registered in this window, for the pickers to offer. */
export function useStoreFontFamilies(): readonly string[] {
  return useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot,
  )
}

/**
 * Register every cut of a family under its catalog name. Resolves false when the
 * faces could not be registered, which leaves the family unregistered on
 * purpose — a half-registered family would fall back for the styles that failed
 * and read as a font bug rather than a failed download.
 */
export async function registerStoreFamily(
  family: string,
  cuts: readonly DocsFontFace[],
): Promise<boolean> {
  if (typeof FontFace === 'undefined' || typeof document === 'undefined') return false
  if (!cuts.length) return false
  const loaded: FontFace[] = []
  for (const cut of cuts) {
    try {
      const desc = STYLE_FACE[cut.style]
      const face = new FontFace(family, sfntBuffer(cut.bytes), desc)
      await face.load()
      loaded.push(face)
    } catch {
      /* an unreadable cut fails the whole family: see the doc comment */
    }
  }
  if (loaded.length !== cuts.length) return false
  // a re-download replaces the old faces rather than stacking a second set
  for (const face of faces.get(family) ?? []) document.fonts.delete(face)
  for (const face of loaded) document.fonts.add(face)
  faces.set(family, loaded)
  publish()
  // availability and every chain's metrics were measured against the fallback
  noteFontFacesChanged([family])
  // buffer-backed faces parse synchronously: the set never enters 'loading', so
  // the measurement caches keyed on 'loadingdone' must be told by hand
  document.fonts.dispatchEvent(new Event('loadingdone'))
  return true
}

/** Ask the main process for one family's cuts and register them. */
async function registerViaApi(family: string): Promise<boolean> {
  try {
    const cuts = await window.desktop?.fontStoreFaces?.(family)
    if (!cuts?.length) return false
    return await registerStoreFamily(family, cuts)
  } catch {
    return false
  }
}

/**
 * Re-register every family already in the store, so a font downloaded in an
 * earlier session is in the picker again. Returns the families that landed.
 */
export async function registerStoredFamilies(): Promise<readonly string[]> {
  let families: readonly string[]
  try {
    families = (await window.desktop?.fontStoreFamilies?.()) ?? []
  } catch {
    return []
  }
  const landed: string[] = []
  for (const family of families) if (await registerViaApi(family)) landed.push(family)
  return landed
}

/**
 * Download one family and register it, reporting whether the reader can now use
 * it. Wrapped around the raw IPC pair rather than replacing it, so the shared
 * hook's busy/failed bookkeeping still owns one in-flight download per family.
 */
export async function downloadAndRegister(
  family: string,
): Promise<{ ok: boolean; error?: string }> {
  const result = await window.desktop?.fontDownload?.(family)
  if (!result?.ok) return { ok: false, error: result?.error ?? 'download failed' }
  return (await registerViaApi(family)) ? { ok: true } : { ok: false, error: 'font unusable' }
}

/** Test seam: forget every registration. */
export function resetStoreFonts(): void {
  faces.clear()
  publish()
}
