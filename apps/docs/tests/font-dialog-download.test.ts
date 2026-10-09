import { describe, expect, it } from 'vitest'
import { availableFontFamilies, catalogFontSlot } from '../src/renderer/components/FontDialog'
import { formatFontBytes } from '@genoffice/ui'

/**
 * The two decisions the download section makes that are not React: which slot a
 * downloaded family lands in, and which families the pickers end up offering.
 * Both are wrong in ways only a test can see — a CJK family in the Latin slot
 * clobbers the document's Chinese font, and a family that never reaches the
 * options is a download the reader cannot use.
 */
describe('catalogFontSlot', () => {
  it('sends Latin families to the Latin slot', () => {
    expect(catalogFontSlot({ script: 'latin' })).toBe('fontLatin')
  })

  it.each(['ja', 'ko', 'sc', 'tc'] as const)(
    'sends the %s family to the East Asian slot',
    (script) => {
      expect(catalogFontSlot({ script })).toBe('fontEastAsia')
    },
  )
})

describe('availableFontFamilies', () => {
  it('lists a downloaded family alongside the machine own', () => {
    expect(availableFontFamilies(['Calibri'], ['Helvetica'], ['Rubik'])).toEqual([
      'Helvetica',
      'Rubik',
    ])
  })

  it('keeps a built-in candidate out of the second list, so it is not listed twice', () => {
    expect(availableFontFamilies(['Calibri', 'SimSun'], ['Calibri', 'Helvetica'], [])).toEqual([
      'Helvetica',
    ])
  })

  it('lists a downloaded family once even when the machine also has it', () => {
    expect(availableFontFamilies([], ['Rubik'], ['Rubik'])).toEqual(['Rubik'])
  })

  it('is empty when there is neither a system font nor a download', () => {
    expect(availableFontFamilies(['Calibri'], [], [])).toEqual([])
  })
})

describe('formatFontBytes on the rows this section shows', () => {
  it('prices the largest published family in MiB, not MB', () => {
    // the catalog counts bytes; "28 MB" would promise disk the write does not use
    expect(formatFontBytes(29_435_021)).toBe('28 MiB')
  })

  it('keeps a decimal under 10 MiB so a small family is not rounded to nothing', () => {
    expect(formatFontBytes(4_404_136)).toBe('4.2 MiB')
  })
})
