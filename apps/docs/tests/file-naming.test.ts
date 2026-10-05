import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { Editor } from '@tiptap/core'
import { TextSelection } from '@tiptap/pm/state'
import { parseDocx } from '@genoffice/docx-engine'
import { buildDocx } from '../../../packages/docx-engine/tests/helpers/build-docx'
import { blocksToPmDoc } from '../src/renderer/editor/convert'
import { editorExtensions } from '../src/renderer/editor/extensions'
import { DocRedaction } from '../src/renderer/editor/redaction'
import {
  dialogNameFor,
  noteDocumentSwapped,
  suggestedSaveAsName,
} from '../src/renderer/file-actions'
import type { FileActionContext } from '../src/renderer/file-actions'

/**
 * The manual trigger: the reader asks the model to name a document that
 * already has a name, and the answer is offered in the Save As dialog.
 *
 * These go through the app's own functions rather than `nameForSave`, because a
 * test of the shared client still passes if this app stops using the
 * projection — which is the failure that leaks a secret.
 *
 * The dialog hop matters as much as the call: a stem that never reaches the
 * dialog is a button that does nothing, and that is exactly what this shipped
 * as before it had a call site.
 */

const LABEL = '客户电话'
const SECRET = '13800138000'
const FILE_NAME = '现有文档.docx'
const ANSWER = '季度营收总结'
const extensions = [...editorExtensions, DocRedaction]
const editors = new Set<Editor>()
afterEach(() => {
  for (const e of editors) e.destroy()
  editors.clear()
})

/** Calls the naming bridge received, in order. */
let sent: Array<{ content: string; trigger: string; filePath: string | null }> = []
/** What the model answers with; '' is a model that returned nothing usable. */
let answer = ANSWER

function installBridge(): void {
  ;(window as unknown as { aiOffice: unknown }).aiOffice = {
    suggestFileName: async (input: {
      content: string
      trigger: string
      filePath?: string | null
    }) => {
      sent.push({
        content: input.content,
        trigger: input.trigger,
        filePath: input.filePath ?? null,
      })
      return answer ? { ok: true, name: answer } : { ok: false, reason: 'empty-name' }
    },
    getFileNamingEnabled: async () => true,
  }
}

interface Opened {
  editor: Editor
  parsed: Awaited<ReturnType<typeof parseDocx>>
}

/** a document with a real parse behind it, so the save path can serialize it */
async function open(text: string): Promise<Opened> {
  const bodyXml = `<w:p><w:r><w:t xml:space="preserve">${text}</w:t></w:r></w:p>`
  const parsed = await parseDocx(await buildDocx({ bodyXml }))
  const editor = new Editor({
    element: document.createElement('div'),
    extensions,
    content: { type: 'doc', content: blocksToPmDoc(parsed.blocks).content as never[] },
  })
  editors.add(editor)
  return { editor, parsed }
}

function selectText(editor: Editor, needle: string): void {
  let start = -1
  editor.state.doc.descendants((node, pos) => {
    if (start >= 0) return
    const at = node.textContent.indexOf(needle)
    if (at >= 0) start = pos + 1 + at
  })
  expect(start).toBeGreaterThan(0)
  editor.view.dispatch(
    editor.state.tr.setSelection(
      TextSelection.create(editor.state.doc, start, start + needle.length),
    ),
  )
}

/** a document that already has a name, holding one withheld span */
async function namedWithSecret(): Promise<Opened> {
  const opened = await open(`联系 ${SECRET} 确认季度营收`)
  selectText(opened.editor, SECRET)
  opened.editor.commands.setRedaction(LABEL)
  return opened
}

/**
 * Only the fields the save path actually reads. `save` takes the whole
 * context, and the rest of it belongs to the window this test is not building.
 */
function ctxFor(opened: Opened, fileName: string): FileActionContext {
  return {
    editor: opened.editor,
    doc: { parsed: opened.parsed, filePath: null, fileName, hash: '', isBlank: false },
    dirtyRef: { current: true },
    saveInFlightRef: { current: false },
    saveIncompleteRef: { current: false },
    pendingMixedExportRef: { current: false },
    setStatus: () => {},
    setRecent: () => {},
    section: null,
    sectionDirty: false,
    sections: [],
    sectionsDirty: [],
    trailingStartType: null,
    setSection: () => {},
    setSections: () => {},
    setSectionDirty: () => {},
  } as unknown as FileActionContext
}

beforeEach(() => {
  sent = []
  answer = ANSWER
  installBridge()
  // the latch is module state; each case starts from a freshly swapped document
  noteDocumentSwapped()
})

describe('naming a document that already has a name', () => {
  it('hands the model the projected text, never the withheld words', async () => {
    const opened = await namedWithSecret()
    await suggestedSaveAsName(ctxFor(opened, FILE_NAME))
    expect(sent).toHaveLength(1)
    expect(sent[0].content).not.toContain(SECRET)
    // and the marker is there, so the model can still tell something was hidden
    expect(sent[0].content).toContain(LABEL)
  })

  it('asks on request, so it does not consult the first-save preference', async () => {
    const opened = await namedWithSecret()
    await suggestedSaveAsName(ctxFor(opened, FILE_NAME))
    expect(sent[0].trigger).toBe('manual')
  })

  it('offers the answer as a complete file name, extension and all', async () => {
    const opened = await namedWithSecret()
    expect(await suggestedSaveAsName(ctxFor(opened, FILE_NAME))).toBe(`${ANSWER}.docx`)
  })

  it('proposes nothing when the model has nothing usable', async () => {
    // '' is the signal that leaves the dialog on the name the document had
    answer = ''
    const opened = await namedWithSecret()
    expect(await suggestedSaveAsName(ctxFor(opened, FILE_NAME))).toBe('')
  })
})

describe('the name the Save As dialog opens on', () => {
  it('leads with what the caller proposed', () => {
    expect(dialogNameFor(`${ANSWER}.docx`, null, FILE_NAME)).toBe(`${ANSWER}.docx`)
  })

  it('falls back to the name derived from the document, then to its own', () => {
    expect(dialogNameFor(undefined, '季度营收.docx', FILE_NAME)).toBe('季度营收.docx')
    expect(dialogNameFor(undefined, null, FILE_NAME)).toBe(FILE_NAME)
  })

  it('lets a proposal win over the derived name', () => {
    expect(dialogNameFor('手动命名.docx', '季度营收.docx', FILE_NAME)).toBe('手动命名.docx')
  })
})
