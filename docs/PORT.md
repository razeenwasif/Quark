# Bringing Proton's Reader and LaTeX into Quark

Proton (`~/Github/Applications/Proton`) has a PDF reader-editor ("Reader")
and an Overleaf-style LaTeX mode. Both are moving to Quark. Once Quark's
versions are as good as Proton's, they will be taken out of Proton, as a
separate step that is confirmed first.

## Decisions (2026-10-27)

- **Copy, don't link.** Proton's crates that have no UI in them are copied
  into Quark's `crates/` and renamed `quark-*`. Quark builds on its own,
  Windows included, and the two apps change independently. The `ts-rs`
  derives, which only fed Proton's web window its TypeScript types, are
  removed.
- **Reader:** PDFium keeps drawing pages. Editing moves to Proton's engine,
  which brings:
  - forms filled and flattened;
  - comments with their own appearances, so other apps show them, plus
    replies and edits;
  - saving that appends changes and keeps the original bytes;
  - undo for every change, even after a save;
  - extract, split and combine into new files;
  - OCR's text layer.

  Quark keeps what it has over Proton:
  - native rendering, pinch zoom and its zoom modes;
  - crop, redaction, watermarks, headers and footers, Bates numbering;
  - export to images and text, PDFs made from images;
  - Finder and Windows integration;
  - its AI backends.
- **Library:** Proton Reader's home page, with shelves per folder from
  `~/Documents/Books`, Continue reading, covers, renaming and reading
  progress. It is drawn natively, with covers rendered by PDFium (no
  Poppler).
- **LaTeX:** the editor uses Vim keys, from Proton's own modal engine. The
  first version has:
  - the editor, with texlab;
  - building, with a clickable log;
  - the PDF in Quark's viewer, with SyncTeX both ways;
  - the TinyTeX install, asked for first;
  - templates and the cite picker;
  - the visual editor.

  TeX runs on macOS and Linux, as in Proton. Windows comes later.

## Where each piece comes from

| Quark crate | From Proton | Notes |
|---|---|---|
| `quark-edit` | `proton-pdf` | lopdf 0.44 (Proton is on 0.42). `pdf-extract` is dropped: text with positions comes from PDFium (`quark-pdf::text`) |
| `quark-text` | `proton-text` | rope buffer, undo tree, motions |
| `quark-modal` | `proton-core`'s `vim`, `action`, `key`, `editor`, `block`, `split`, `ex`, `typing` | copied out of the core |
| `quark-syntax` | `proton-syntax` | LaTeX and BibTeX grammars only; no wasm |
| `quark-lsp` | `proton-lsp`, plus `proton-core`'s `lsp.rs` | texlab |
| `quark-tex` | `proton-tex` | log, project, SyncTeX, bib, blocks, outline, templates |
| `quark-lab` | `proton-lab`'s `tex/`, `ocr`, `run`, `sandbox`, `job`, `github` | TinyTeX install and builds; OCR with tesseract |
| `quark-literature` | `proton-literature`, plus the record types from `proton-research` | the cite picker's online search |

## Milestones

Each one lands on the `proton-port` branch with its tests passing.

| | Builds | Proved by |
|---|---|---|
| **Q0** | `quark-edit`, `quark-text`, `quark-tex`, `quark-lsp` and `quark-lab` copied in, compiling and passing their own tests | the copied tests |
| **R1** | The editing engine behind Quark's viewer:<br>- comments made, edited, replied to and deleted, drawn so other apps show them<br>- forms filled and flattened<br>- appended saves; undo and redo of every change<br>- a prompt on quit with unsaved work<br>- fixes for comments that come back after deleting and for colliding ids | `quark-pdf`'s service tests, `quark-app`'s tab tests |
| **R2** | The library home page: shelves, Continue reading, PDFium covers, renaming, progress, filtering | a walker over a fixture folder; the cover cache |
| **R3** | The rest of Reader:<br>- page labels and web links<br>- a page grid<br>- extract and split to new files<br>- removing passwords<br>- one page at a time<br>- OCR<br>- sharp rendering on Retina | engine and service tests |
| **L1** | The LaTeX editor: `quark-text` and `quark-modal` in a native code editor (lines, syntax, cursor, selection, Vim modes, splits, `:w`) over a project's files | the modal tests; editor layout tests |
| **L2** | TinyTeX: install cards with sizes, asked for first; texlab completion and diagnostics | `quark-lab`'s tests with stand-in tools |
| **L3** | Builds:<br>- latexmk on save<br>- the log, each error a click from its line<br>- the PDF in Quark's viewer, read again in place<br>- SyncTeX: ⌘↩ forward, double-click back<br>- the outline | the build tests with a stand-in latexmk |
| **L4** | Templates, and the cite picker: the `.bib`, then online | template and bib tests; fixture searches |
| **L5** | The visual editor: Proton's blocks drawn natively, text edited in place, math rendered by TinyTeX into images | block round trips |
| **X** | LaTeX and Reader taken out of Proton, once you confirm | Proton's tests |
