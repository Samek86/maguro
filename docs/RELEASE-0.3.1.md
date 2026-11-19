# Maguro 0.3.1 release notes

A small fix release on top of 0.3.0.

## Fixes

- **No flicker when switching tabs.** Switching window tabs (click, Ctrl+Tab, Ctrl+Shift+Tab) now paints the new tab in one frame. The previous tab's panes no longer show through for a moment, and the folder view appears once it has finished loading. Fast Ctrl+Tab presses are coalesced, so cycling through several tabs stays smooth.
- **Memory stays the same as 0.3.0.** Only two hidden tabs keep their folder views cached, as before, so memory use does not grow with this fix.
