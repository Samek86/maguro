# Maguro 0.3.4 release notes

A preview-focused release on top of 0.3.3.

## Quick Look (F3 / Space) and the preview pane (Alt+P)

- **Zoom and text selection.** Zoom in and out, and select and copy text from the preview.
- **Encoding detection.** Text files without a BOM (UTF-8, UTF-16, Shift_JIS, EUC-KR, Windows-1252) are detected and shown correctly.
- **SVG.** SVG files are rendered as images, with the source as a fallback.
- **More formats.** CSV/TSV as a table, Markdown, RTF, ODT/ODS, EPUB, EML/MSG mail, LNK/URL shortcuts, and TAR/GZ/CAB archive listings.
- **More pictures.** WebP, HEIC, AVIF, ICO and DDS. Small pictures are scaled to fit the window.
- **Built-in audio and video playback.** Audio and video play with Windows Media Foundation, no codecs bundled. A registered preview handler is used only when the built-in player can't open the file.
- **Excel preview fix.** Spreadsheet previews no longer close by themselves.

## Interface

- **Address bar.** Removed the "open in another pane" button next to the address bar. The New pane tab button now sits right of Up.
- **Toolbar editor.** The toolbar editor opens 1.5× wider and one-third shorter.

## Fixes

- **German and French.** The "Open in Maguro" menu item is now translated.