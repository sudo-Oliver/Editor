# UI specification

The layout follows Overleaf's dark editor. Values were measured from a screenshot and
the accessibility tree of the live page (REA evidence
`ev_f03409650cb77c25d512f9c7575cf71e3799a80a9f31d6bc8e19d5eeda95b72b` and
`ev_9c91f6d58135ba7d946a20ea86557c63db57b6fda6c362fc6cd4634cdef07bcb`) at a viewport of
1496 x 1591 CSS pixels. Only layout, measurements and colors are reused. Overleaf's
logo, icons and code are not.

## Measurements (CSS pixels)

| Element | Value |
| --- | --- |
| Top bar height | 39 plus 1 border |
| Left rail width | 39 plus 1 border |
| File panel width | 217.5 |
| Pane dividers | 7 wide |
| File tree / outline divider | 6 high |
| Editor tab bar | y 40 to 76 |
| Editor toolbar | y 76 to 108 |
| PDF toolbar | y 40 to 72 |
| Code font | 12, line height 19.2 (inferred from 24 px rows in the scaled screenshot) |

The code font size is inferred. The capture has no computed styles.

## Colors

| Role | Value |
| --- | --- |
| Base background | `#1c222b` |
| Toolbar and PDF background | `#313a4b` |
| Dividers, inactive toggle | `#4b5364` |
| Active line | `#444759` |
| Primary green | `#3b8549` |
| Selected row | `#2c5739` |
| UI text | `#f4f5f6` |
| Code text | `#f8f8f3` |
| Line numbers | `#909194` |
| Comments | `#6472a1` |
| Cyan commands, italic | `#9fe6fb` |
| Pink commands | `#ef83c3` |
| Orange arguments, italic | `#f5bb77` |
| Spelling or error squiggle | `#ec381e` |
| Caret | `#4a9cd4` |

## Settings and shortcuts dialogs

The bottom of the rail holds the question mark above the gear, as in Overleaf. Observed on
the live page: a centered modal over the dimmed app, about 640 x 560 CSS pixels, with tabs
on the left and rows of title, description and control on the right. Only the Editor tab
and the rail were inspected. The other tabs and Overleaf's help dialog were not, and its
latency was not measured.

The dialog here uses the app's dark colors and lists only settings that work. Choices stay
visible as segments instead of a combobox, so that one click changes them.

## Editor toolbar

The toolbar follows the file type. LaTeX and Typst show undo, redo, bold, italic, math, link,
the symbol palette, the table grid, figure and citation. Markdown keeps undo, redo, bold,
italic and link. Other files and the image preview keep undo and redo.

## Git panel

The second rail button swaps the file tree and outline for the panel, at the same width. From
the top: branch with ahead and behind counts, pull, push, refresh; conflicts, staged files and
changes; the commit message and button; history with one lane per branch.

## Add-on panels

The Add-ons rail button below Git lists installed add-ons with On, Off, Remove and Install from
folder. An add-on that is turned on gets a rail button of its own below that, with its own icon, and
its panel takes the place of the file tree. The add-on sends a tree of nodes, so its panel uses the colors and controls of the app.

The same panel has two sections: Installed, and Available from the catalog with description,
permissions, Install and Refresh. Its text says that downloads are checked but not signed. An update
appears on the installed add-on when the catalog holds another version. Installing never turns an
add-on on.

## DOI dialog

An add-on with the `lookup_doi` permission draws a dialog over the top of the window, opened with Cmd+Shift+D, Insert > Citation from a DOI, or `Space r` in Vim mode. Enter presses its default button, and Esc closes it and drops an answer still on its way. A dialog without a default button, as for a retracted work, ignores Enter.

## Citation picker

`\cite{` and its relatives open the picker in LaTeX when entries exist. Backspace after a pick does not
open it again at that place. A pick inserts the key, adds `}` if none follows, and leaves the cursor
after the brace. An open picker takes the keyboard in every mode.

## Vim status bar

The status bar exists only in Vim mode. It follows the NvChad statusline with
`theme = "default"` in the tokyodark colors of base46. The rest of the app keeps the colors
above. The separators are drawn as paths, so no Nerd Font is needed.

The Vim, position and file type icons are glyphs from Nerd Fonts, stored as SVG files. The
mapping from file type to icon and color follows `nvim-web-devicons`.

| Element | Color |
| --- | --- |
| Normal | `#648ce1` |
| Insert | `#9071c9` |
| Visual, Visual Line | `#38a89d` |
| Command, position | `#98c379` |
| Background | `#161722` |
| File segment | `#2a2b36` |
| Text | `#a0a8cd` |
| Errors, folder | `#ee6d85` |
| Warnings | `#d7a65f` |
| Search hit | `#6b5a1e` |
