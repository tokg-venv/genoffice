# Slicer persistence sample v2 (PR #2226, review round 3)

Generated outside the repository, through the same pipeline the maintainer used:
openpyxl blank workbook -> gateway pivotAdditions -> gateway slicerState -> save.

## Files

- /tmp/slicer-sample2.xlsx - the fresh sample for the Excel gate (after the URI/x14-id fixes).
- /tmp/go2226-blank.xlsx - openpyxl input (one Data sheet: header Region/Product/Amount, rows East/A/100, West/B/50, East/B/50 = A1:C4).
- /tmp/go2226-with-pivot2.xlsx - intermediate after the pivotAdditions save.
- /tmp/go2226-sample-gen.ts - the tsx driver script for the gateway planner.
- /tmp/go2226-venv/ - throwaway venv with openpyxl 3.1.5 (system python3 is externally managed).
- /tmp/slicer-sample.xlsx + /tmp/slicer-sample.md - the previous round's sample, kept for comparison.

## How it was produced

1. Blank workbook with data via openpyxl (openpyxl writes the self-closing <definedNames /> shape):

       python3 -m venv /tmp/go2226-venv
       /tmp/go2226-venv/bin/pip install openpyxl
       /tmp/go2226-venv/bin/python /tmp/go2226-make-blank.py   # writes /tmp/go2226-blank.xlsx

2. One planner call for the pivot addition (same positional API the gateway tests use),
   saved to /tmp/go2226-with-pivot2.xlsx:

       planCellEditsToXlsx(source, [], ..., pivotAdditions=[{ sheetName:'Data',
         sourceSheetName:'Data', sourceArea A1:C4, location E1:G4, name:'PivotData',
         fieldNames:['Region','Product','Amount'], rowFieldIndices:[0],
         rowItems:['East','West'], values:[{fieldIndex:2, agg:'sum'}] }])

3. A second planner call on that saved package with the slicer panel, saved to
   /tmp/slicer-sample2.xlsx:

       slicerState = { slicers: [{ sheetName:'Data',
         pivotPath:'xl/pivotTables/pivotTable1.xml', sourceName:'Region',
         caption:'Region', items:[{x:0,selected:true},{x:1,selected:false}] }],
         removals: [] }

   Run: cd <worktree> && npx tsx /tmp/go2226-sample-gen.ts (copy the script into the
   worktree first: jszip resolves from the repo node_modules, not from /tmp).

## What changed since the previous sample (review items 1 and 2)

- xl/worksheets/sheet1.xml extLst now uses the MS-XLSX 2.2.2 URI
  {A8765BA9-456A-4dab-B4F3-ACF838C121DE} (previously the wrong
  {A8765BA9-456A-4dab-B4F3-ACF1056F45AA}). This is the reported blocker: Excel treated
  the sheet ext as foreign, never instantiated the slicer, then discarded the slicerCache
  and the drawing on Save As. The constant is now pinned by a test (writer output has the
  exact string; the bogus one is asserted absent) and both addExtRef/removeExtRef key on it,
  so Excel-authored files with the real ext are found and cleaned instead of getting a
  second/dangling ext.
- x14 pivotCacheId is treated as its own id space (review item 2). In this sample the
  workbook pivotCache@cacheId is 1, the definition's x14 pivotCacheId is 1, and the slicer
  cache's tabular@pivotCacheId is 1 - the writer allocated the first x14 id because the
  definition had no ext. For a genuine Excel file (cacheId=110, x14 id=2124480673) the
  reader resolves through the definitions' x14 exts and the writer reuses the existing id;
  the workbook-cacheId fallback only applies to definitions with no ext at all. Covered by
  gateway tests (resolve, legacy fallback, no-tabId-guess, reuse-on-rewrite).
- Removed the tabId sorted-part fallback (review item 3): tabId is a sheetId, never a cache
  discriminator.
- A slicerCache rewrite now keeps the file's whole <pivotTables> block (review item 4), so an
  Excel slicer connected to several pivots (Report Connections) keeps all of them; covered by
  a gateway test with two pivotTables entries.
- Stale comments about slicerData / persisted timelines fixed (review item 5).

## Package contents (verified locally)

- xl/workbook.xml: <definedNames><definedName name="Slicer_Region" hidden="1">#N/A</definedName></definedNames>
  after </sheets> and before <calcPr>/<pivotCaches>; workbook extLst
  {BBE1A952-AA13-448e-AADC-164F8A28A991} -> <x14:slicerCaches><x14:slicerCache r:id="rId5"/></x14:slicerCaches>.
- xl/_rels/workbook.xml.rels: slicerCache relationship.
- xl/slicerCaches/slicerCache1.xml: name=Slicer_Region, sourceName=Region (matches the pivot
  cache field), <pivotTable tabId="1" name="PivotData"/> (tabId = pivot host sheetId, name =
  pivot table name), <tabular pivotCacheId="1" crossFilter="showItemsWithDataAtTop"> with items
  x=0 selected / x=1 not.
- xl/pivotCache/pivotCacheDefinition1.xml: x14 ext <x14:pivotCacheDefinition pivotCacheId="1"/>,
  no slicerData (that flag is OLAP-only; MUST be absent/false for a tabular cache).
- xl/worksheets/sheet1.xml: slicer rel (http://schemas.microsoft.com/office/2007/relationships/slicer)
  + drawing rel; extLst x14:slicerList -> <x14:slicer r:id="rId2"/> under the real URI;
  <drawing r:id="rId3"/> before the extLst.
- xl/drawings/drawing1.xml: twoCellAnchor(editAs="oneCell") -> mc:AlternateContent
  (mc:Choice Requires="sle" with graphicFrame + sle:slicer, mc:Fallback xdr:sp) ->
  <xdr:clientData/> as the anchor's last child (not inside the graphicFrame).
- xl/slicers/slicer1.xml: name=Region, cache=Slicer_Region, caption=Region, rowHeight=241300.
- [Content_Types].xml: slicer + slicerCache + drawing overrides.

## Verification performed here

- zip integrity OK; all 18 XML/.rels parts parse (python zipfile + minidom).
- No part contains slicerData; exactly one <definedNames> with #N/A; worksheet ext URI is the
  real one and the old bogus GUID is absent; clientData is at anchor level only; content types
  carry all three overrides; the package re-reads through parseSlicerState (pivot resolved).
- Gateway suite 405/405 tests, slicer round-trip 22/22; apps/sheets slicer/restore 11/11;
  tsc clean in both packages; eslint/prettier/check-english-comments clean.
- NOT verified: Excel open -> Save As -> reopen. Excel is not installed on this Mac and
  LibreOffice does not emulate Excel's repair/slicer-binding behaviour, so the maintainer's
  Excel gate (whether slicerCache1.xml + drawing1.xml + slicer1.xml survive Save As, with Excel
  taking ownership of the cache and adding xr10:uid) remains the final check.
