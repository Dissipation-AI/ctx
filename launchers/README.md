# Scoped launchers

`launcher.c` and `build.sh` are the templates `ctx make launcher` fills in. A launcher starts one
program in one folder under its own macOS identity, so a permission granted to it (such as Full
Disk Access) reaches only that program. See SPEC.md, "Permissions → Making launchers".

The template is reviewed once and tested on every macOS release. Generated launchers live in
`~/Library/Application Support/ctx/launchers/<name>/` with their filled-in source and build
script next to the binary.
