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
| Editor tabs | 32 high, interlocked by edges with 12 px slant and a 1.5 px gap. Edges lean toward the active tab, which has the toolbar color and a 1 px top line |
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

## Package hints

Hints from the editor use a muted underline, so that they do not read as compiler errors. They add
to neither the error count nor the badge. The error card shows a primary button with the fix, and
the log panel lists all hints in a Packages section, each with its button. With the cursor on a
hinted line, a strip below the editor says what the underline means and applies the fix on click.

## File tree header

Clicking the header folds the tree, and the outline takes its space. To the right sit four 22 px
buttons: new file, new folder, add files and close panel. New entries go to the last clicked folder,
or the folder of the last clicked file, or else the project root. In Vim's tree that is the row under
the cursor. A name field opens below the header, with `Enter` to create and `Esc` to cancel. Nothing
gets replaced: an existing name shows an error below the field, and added files with an existing name
are left out and named in the same place.

## Suggestion popup

A small list opens below the start of the replaced text, at most 8 rows of name and detail on the
panel color. It appears only at a trigger (`\\`, `\\begin{`, `\\ref{`, `#`, `@`), not in prose. Its
keys apply only while it shows, so `Tab` keeps its other meanings otherwise: indent or next field
while inserting, and file tree in Vim's Normal mode.

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

## Add-on dropdowns

A dropdown is a quiet button with the chosen option and a chevron. Its list opens upward, 200 wide, in the panel color with a divider border, and marks the chosen option with a check. Arrows or `j` and `k` move, Enter chooses, and Esc or a click elsewhere closes it and gives the focus back.

## File menu

Menu rows keep their size. A separator is 1 high with 4 above and below, in the divider color. A heading such as `Download` is muted and takes no clicks. Rows under it are not indented.

Word count uses the dialog of the settings, without its side tabs. The total comes first in semibold, then the limits of the count, then the words per section, so that the limits show without scrolling.

Export questions use the system's alert, and the target comes from the system's save dialog. The safe or usual choice is the first button. The status in the preview toolbar shows the running step and the result.

## DOI dialog

An add-on with the `lookup_doi` permission draws a dialog over the top of the window, opened with Cmd+Shift+D, Insert > Citation from a DOI, or `Space r` in Vim mode. Enter presses its default button, and Esc closes it and drops an answer still on its way. A dialog without a default button, as for a retracted work, ignores Enter.

## Citation picker

`\cite{` and its relatives open the picker in LaTeX when entries exist, and so does a comma in the
key list, a further group of `\cites{a}{`, and `@` or `#cite(<` in Typst. Backspace after a pick does
not open it again at that place. Keys the citation already has are left out of the list. A pick
inserts the key, adds `}` if none follows, and leaves the cursor after the brace; Cmd+Enter inserts
the key and a comma and keeps the picker open for the next one. Without a query the list shows the
first 200 entries by author and year, and the header says how many there are.

Beside the list, a panel shows the chosen entry in full: all authors, title in semibold, venue and
year, DOI or URL, the start of the abstract, the entry type and file:line. It uses the palette, so
paper mode looks like the editor. The entries come from the citation index, which reads only the
bibliographies the document declares and rebuilds off the main thread after changes.

An open picker takes the keyboard in every mode.

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
