# Slicer persistence sample — PR #2226

Generated outside the repository, through the same pipeline the reviewer used:

    openpyxl blank workbook -> gateway pivotAdditions -> gateway slicerState -> save

## Files

- /tmp/slicer-sample.xlsx — the sample to attach to the PR (after the review fixes).
- /tmp/go2226-blank.xlsx — the openpyxl input (one "Data" sheet: Region/Product/Amount, rows 2-4).
- /tmp/go2226-with-pivot.xlsx — intermediate after the pivotAdditions save.
- /tmp/go2226-sample-gen.ts — the gateway driver script (tsx).
- /tmp/go2226-venv/ — throwaway venv with openpyxl 3.1.5 (system python3 is externally managed).

## How it was produced (on this machine)

1. Blank workbook with data via openpyxl (openpyxl writes the self-closing
   `<definedNames />` shape the review mentions):

       python3 -m venv /tmp/go2226-venv
       /tmp/go2226-venv/bin/pip install openpyxl
       /tmp/go2226-venv/bin/python /tmp/go2226-make-blank.py   # writes /tmp/go2226-blank.xlsx

   Sheet "Data": header Region/Product/Amount, rows East/A/100, West/B/50, East/B/50
   (= A1:C4).

2. First save on the gateway planner — the pivot addition (same positional API the
   gateway tests call; the reviewer's pipeline):

       planCellEditsToXlsx(source, [], ..., pivotAdditions=[{ sheetName:'Data',
         sourceSheetName:'Data', sourceArea A1:C4, location E1:G4, name:'PivotData',
         fieldNames:['Region','Product','Amount'], rowFieldIndices:[0],
         rowItems:['East','West'], values:[{fieldIndex:2, agg:'sum'}] }])
       -> /tmp/go2226-with-pivot.xlsx

3. Second save on the same planner with a slicerState (the new panel):

       slicerState = { slicers: [{ sheetName:'Data',
         pivotPath:'xl/pivotTables/pivotTable1.xml', sourceName:'Region',
         caption:'Region', items:[{x:0,selected:true},{x:1,selected:false}] }],
         removals: [] }
       -> /tmp/slicer-sample.xlsx

   Run: `cd <worktree> && npx tsx /tmp/go2226-sample-gen.ts` (copy it into the
   worktree first if module resolution from /tmp fails: jszip is resolved from the
   repo's node_modules).

## What is inside (binding chain)

- xl/workbook.xml: `<definedNames><definedName name="Slicer_Region" hidden="1">#N/A</definedName></definedNames>`
  inserted after `</sheets>` (openpyxl's self-closing container was opened up, not
  duplicated), before `<calcPr>`/`<pivotCaches>`; workbook extLst
  `{BBE1A952-AA13-448e-AADC-164F8A28A991}` -> `<x14:slicerCaches><x14:slicerCache r:id="rId5"/>`.
- xl/_rels/workbook.xml.rels: slicerCache rel `application/.../relationships/slicerCache`.
- xl/slicerCaches/slicerCache1.xml: name=Slicer_Region, sourceName=Region (matches
  the pivot cache field), `<pivotTable tabId="1" name="PivotData"/>` (tabId = pivot
  host sheetId, name = pivot table), `<tabular pivotCacheId="1" crossFilter=...>`
  with items x=0 selected / x=1 not.
- xl/pivotCache/pivotCacheDefinition1.xml: x14 ext
  `<x14:pivotCacheDefinition pivotCacheId="1"/>` — no slicerData (see below).
- xl/worksheets/sheet1.xml: worksheet rels carry the slicer rel
  (`http://schemas.microsoft.com/office/2007/relationships/slicer`) + drawing rel;
  worksheet extLst `{A8765BA9-...}` -> `<x14:slicerList><x14:slicer r:id="rId2"/>`;
  `<drawing r:id="rId3"/>` before the extLst.
- xl/drawings/drawing1.xml: twoCellAnchor(editAs="oneCell") -> mc:AlternateContent
  (mc:Choice Requires="sle" with graphicFrame + sle:slicer, mc:Fallback xdr:sp) ->
  `<xdr:clientData/>` as the ANCHOR's last child (not inside the graphicFrame).
- xl/slicers/slicer1.xml: name=Region, cache=Slicer_Region, caption=Region,
  rowHeight=241300.
- [Content_Types].xml: slicer + slicerCache + drawing overrides.

## Review items addressed in this sample

1. x14 pivotCacheDefinition ext carries only pivotCacheId; no slicerData. Per
   MS-XLSX 2.6.33 / 2.3.2.1 the slicerData flag must be "false" for non-OLAP
   slicer sources (ours are worksheet pivots) — `slicerData="1"` made Excel report
   "Removed Feature: Slicer Cache". pivotCacheId stays non-zero, as required when
   a slicer cache uses the PivotCache.
2. drawing graphicFrame: `<xdr:clientData/>` removed from inside the frame and
   placed at anchor level after `</mc:AlternateContent>` (Excel repair:
   "Repaired Records: Drawing").
3. The Save-As drop is expected to follow from (1): Excel dropped the slicer cache
   as a repair and then wrote out only the orphaned slicer part, which in turn
   orphaned the drawing frame. The package now declares the non-OLAP binding the
   spec asks for: tabular@pivotCacheId == x14 pivotCacheId == workbook
   pivotCache@cacheId (all "1"), sourceName == cacheField name, tabId == pivot
   host sheetId, pivot table name == pivotTable name.
4. openpyxl's `<definedNames />` is opened up in place (exactly one container) and
   the definedName value is `#N/A`, as Excel writes it; repeating a save keeps a
   single entry.
5. Controls are hosted by the sheet named in the worksheet rel (the sample is
   single-sheet; the dashboard case — slicer on a sheet other than its pivot — is
   covered by gateway + restricted-restore tests).

## Verification performed here

- zip integrity + every XML/.rels part parses (18/18) — python zipfile + minidom.
- The saved package re-reads through parseSlicerState (sheet-1, slicer1.xml /
  slicerCache1.xml, pivotCacheDefinition1.xml, selectedItems [0]).
- Gateway + renderer tests cover the round trip (write -> read -> save again with
  no duplicate rel/ext/frame, no second <definedNames>, drawing/clientData shape).
- NOT verified: Excel opens / Save As / reopen. Excel is not installed on this
  Mac; LibreOffice does not emulate Excel's repair and slicer-binding behaviour,
  so it cannot stand in. The reviewer's Excel-for-Mac check remains the final gate.

## Caveat

The gateway writes generic non-OLAP slicer parts; files that Excel itself authored
keep their geometry/style/ordering flags (style, sortOrder, customListSort,
extLst, xr10:uid, …) when rewritten — our own output only carries what the session
models plus Excel's defaults.

