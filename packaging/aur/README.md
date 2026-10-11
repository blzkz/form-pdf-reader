# AUR packages

Two packages for the [Arch User Repository](https://aur.archlinux.org):

- `form-pdf-reader-bin/`: repackages the x86_64 build attached to each GitHub
  release. Installs in seconds, needs no Rust.
- `form-pdf-reader/`: builds from the release sources with cargo. PDFium (V8 +
  XFA) comes prebuilt from pdfium-binaries, as in the project's own builds,
  checked by its SHA-256.

Each folder has the `PKGBUILD` and the `.SRCINFO` that AUR requires.

## First publication

1. Create an account at https://aur.archlinux.org and add your SSH public key
   in *My Account*.
2. For each package, clone its (empty) AUR repository, copy the two files and
   push:

   ```bash
   git clone ssh://aur@aur.archlinux.org/form-pdf-reader-bin.git aur-bin
   cp packaging/aur/form-pdf-reader-bin/{PKGBUILD,.SRCINFO} aur-bin/
   cd aur-bin && git add PKGBUILD .SRCINFO && git commit -m "Initial upload: 0.3.3" && git push
   ```

   The same with `form-pdf-reader`.

## New version

1. Change `pkgver` (and set `pkgrel=1`) in both `PKGBUILD`s.
2. Update the checksums and the `.SRCINFO`:

   ```bash
   cd packaging/aur/form-pdf-reader-bin
   updpkgsums && makepkg --printsrcinfo > .SRCINFO
   ```

   `updpkgsums` is in `pacman-contrib`. In `form-pdf-reader`, only the first
   checksum (the sources) changes, unless PDFium is updated.
3. Check that it builds (`makepkg -f`), then copy both files to the AUR
   repository and push, as above.
