# Maguro 0.3.6 release notes

A fix and quality-of-life release on top of 0.3.5.

## Fixes

- **Keyboard focus after clicking a pane header.** Clicking a pane number or header now moves keyboard focus into that pane, so Ctrl+V and other shortcuts apply there. Modifier keys alone no longer jump focus back to the previous pane.
- **Copy, paste and drop stay responsive.** Normal copy, move, paste and drag-and-drop run on a background thread, so the window no longer freezes during large transfers.
- **Admin prompt for protected folders.** Copy, move, delete, Shift+Delete, rename and Fast copy now show the Windows elevation prompt when a protected folder such as C:\ denies access. Fast move no longer deletes sources that were not copied.

## Improvements

- **Fast copy progress.** Large-file Fast copy now shows live progress through CopyFileExW.
- **Pane zoom is easier to find.** The pane-number tooltip mentions the double-click shortcut, and the pane-header context menu includes Zoom this pane / Restore layout (Ctrl+Shift+M).
