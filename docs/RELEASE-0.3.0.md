# Maguro 0.3.0 release notes

This is the first release that includes the customizable command toolbar. It is a documentation draft for the GitHub release; do not publish a tag or a GitHub release until Jun gives a final OK.

## Highlights

- **Customizable command toolbar.** Icon-only commands in a vertical column (when window tabs are on the top row) or a horizontal bar (when vertical window tabs are on). About 99 bundled icons. A **More** overflow holds buttons that do not fit.
- **Toolbar editor.** Drag commands between the bar and the palette, rearrange them, and add Open targets for a specific folder or file. Those targets replace the generic Open item. The editor is resizable and has a fixed minimum size. Truncated palette labels show an ellipsis; the tooltip has the full name.
- **Toggle buttons.** An on toggle (for example Show hidden files) keeps the same background as the other buttons and paints only the icon in the accent colour, including hover and press.
- **Focus and tooltips.** After a toolbar command, arrow keys move the list selection again. Tracking tooltips are dismissed on click, when another window is activated, and before a command such as Open in Terminal changes the foreground.

## Also in this line of work

The toolbar work sits on top of the 0.2.25 baseline and also includes the vertical command column, the horizontal bar, the icon pack, opening preset folders and files from the bar, palette label layout, and several focus and tooltip fixes.

## Notes

- The generic Open toolbar item is hidden from the palette once an Open-folder or Open-file target is on the bar.
- Show hidden files remains the same Explorer-wide switch as before (Ctrl+H).
- The built-in default bar is New folder, New text file, Delete, Paste, Rename, View, Properties, Mark as important, a separator, Show hidden files, Duplicate pane to the right, Duplicate pane below, Close pane, and Open Terminal. A toolbar already saved in settings is left as it is; use **Use Default Set** in the editor to apply the new default.

