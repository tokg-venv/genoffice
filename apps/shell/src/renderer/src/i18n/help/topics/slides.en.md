# Slides: presentations

Slides is the PowerPoint-like editor: reads and writes genuine .pptx.

## The interface

- **Ribbon**: the Home tab carries insert & format (text box/shape/picture/table/chart), font & paragraph, align & arrange (z-order, align, distribute).
- **Thumbnail rail** (left): click to switch, drag to reorder, context menu for new/duplicate/delete.
- **Canvas**: WYSIWYG editing; drag, scale handles, guides.
- **Notes**: per-slide speaker notes, kept in export flows.

## Content

- Text boxes, shapes (fill/stroke/shadow), pictures (crop/replace), tables, charts (column/bar/line/pie..., editable data).
- Masters and layouts: unify fonts, placeholders and backgrounds; new slides inherit the chosen layout.
- Theme colors and fonts follow the theme.

## AI generation

- Home's AI Slides card: give a topic or outline and the AI builds the deck; cloud generation falls back to local generation on failure.
- Keep adjusting with the AI panel afterwards (the AI restyles through a controlled script sandbox — same class of mechanism as in the Sheets chapter).

## Present and export

- **Export to PDF**: page-by-page rasterization in a hidden window, with progress and a timeout watchdog for big decks.
- Image export: PNG per page.
- Full-screen presenting where the version provides it.

## Saving

Full .pptx round-trip: untouched pages stay byte-identical; notes, masters and annotations persist.
