# Maguro 0.3.2 release notes

A small release on top of 0.3.1.

## New

- **Uninstall button.** Settings → About now has an Uninstall button under Check for updates. After you confirm, Maguro turns off its "default folder app" registration (current user only), closes, and a hidden cleanup script removes the `%APPDATA%\Maguro` settings folder and `maguro.exe`, then deletes itself. In portable mode (`portable.txt` next to the exe), only Maguro's own settings files beside the exe are removed and the folder itself is kept. Settings of the original grids app are never touched.

## Fixes

- **About credits on their own line.** On Settings → About, the icon credit ("Line icons redrawn from IconMCP shapes…") now appears on its own row instead of running into "© 2026 efww, Samek86".