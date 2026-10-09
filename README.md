# Form PDF Reader

**Read, fill in and save dynamic XFA forms on Linux, without Adobe Reader on
Windows, a virtual machine or Wine.**

Many official forms are dynamic XFA forms (Adobe LiveCycle). Other PDF
viewers only show a "Please wait… / Adobe Reader 8 or higher is required"
page, or render the form without running its scripts. Form PDF Reader runs
those forms properly: dropdowns get their options, sections appear and
disappear, validations work, and the result is saved in a way Adobe Reader
accepts.

It is also an ordinary PDF reader with tabs, text selection and several page
layouts. It is written in Rust with [egui](https://github.com/emilk/egui),
on top of Chromium's **PDFium** engine built with V8 and XFA.

It was created to fill in the Banco de España job application form, which no
free Linux viewer could handle.

The interface is available in English and Spanish. It follows the system
language (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`) and falls back to
English. Set `FORM_PDF_READER_LANG=en` or `es` to force one. Translations
live in `crates/core/locales/<code>.txt`; adding a language means adding a
file there and registering it in `crates/core/src/i18n.rs`. Texts that belong to a form itself (its
labels, messages and validations) stay in the form's own language.

## Features

- **Reading:** normal PDFs with zoom, text search, tabs, and continuous,
  single page or two-column layouts.
- **Tools:**
  - *Form* (default): fill in fields and press form buttons.
  - *Text*: drag to select text and copy it with Ctrl+C.
  - *Hand*: drag to scroll.
- **Forms:** AcroForm and XFA, static and dynamic. That covers text, dates,
  dropdowns, checkboxes, add/remove row buttons, and the form's own
  JavaScript, including validations and messages.
- **Attachments:** the form's own attachment section works (add, view,
  delete), with its size and type limits. Files are embedded in the PDF the
  same way Acrobat does it. "View" opens PDF attachments in a new tab.
  "Archivo > Adjuntos del PDF…" lists the embedded files of any PDF.
- **Saving:** Ctrl+S and Save As, with a warning about unsaved changes.
  Saving keeps the original file intact (see below).
- **Export and print:** a "flat" (image) PDF on A4 pages. Printing opens that
  PDF in the system viewer.

## How dynamic XFA forms are handled

PDFium can run XFA, but it has bugs with complex forms. The application
applies these fixes **only to the in-memory copy** that PDFium sees. The file
on disk is never altered.

1. **Continuous view.** PDFium's XFA pagination loses and reorders sections,
   so the form is shown as a single long page, like a web form.
2. **Script references.** Adobe resolves `xfa.resolveNode("Name")` with a
   deep search and PDFium does not; without this fix dropdowns stay empty.
3. **Widget relayout.** After sections appear or rows are added, PDFium
   leaves controls at their old position. The cause is in
   `CXFA_FFNotify::OnLayoutItemAdded`, which compares a reference with
   itself. An invisible button injected into the template forces a relayout.
4. **Attachments.** The form uses Acrobat-only functions (`importDataObject`,
   `dataObjects`, `getDataObject`, `removeDataObject`, `exportDataObject`).
   They are replaced by an object that talks to the application through
   `app.response()`.
5. **Read-only buttons.** Adobe runs clicks on buttons with
   `access="readOnly"` (View, Delete) and PDFium does not. Their click event
   is run with `execEvent("click")`.
6. **Removing rows.** PDFium does not relayout after `removeInstance()`. The
   row is hidden first and a relayout is forced.
7. **Translated labels.** The form framework calls `exData.loadXML()`, which
   PDFium lacks, and the exception aborted whole scripts. It is replaced by a
   working version.
8. **Cascading dropdowns.** PDFium fires `change` again with empty text when
   a script assigns `rawValue`, and also when a script fills a list. Only
   changes on the focused dropdown are processed, and dependent lists are
   filled when the PDF is opened, as Adobe does.
9. **Dates.** Stored as DD/MM/YYYY, as Adobe does with these forms; the
   form's validation requires it. The cell "comb", which PDFium draws with
   white stripes, is removed.
10. **Formatting and validation on exit.** Forms do this with
    `listen="refAndDescendents"` events, which PDFium ignores. They are
    copied to every field: text becomes uppercase and calendar dates become
    DD/MM/YYYY, as in Adobe.
11. **Script objects.** PDFium mixes up the variables of script objects with
    the same name. Each one is wrapped in a function to isolate them.
12. **User changes only.** PDFium fires `change` when a script assigns a
    field, for instance on open, and several form scripts erased data on that
    spurious event. Change scripts now run only when the field has focus.
13. **Form state.** Adobe stores which fields are open, which sections are
    visible and the value of unbound checkboxes, and PDFium does not restore
    it. The application restores it from Adobe's `form` packet or from its
    own copy, written when saving.
14. **subformSet.** PDFium makes everything inside a `subformSet`
    non-interactive. It is replaced by an equivalent subform.
15. **Re-running initialize.** After a user change on a dropdown, Adobe runs
    its `initialize` script again, and some forms depend on it. The
    application does the same.

The interface also works around some PDFium behaviour:

- Clicking a field's label moves focus to the field, as in Adobe. Otherwise
  PDFium leaves focus half-set and typed text ends up in another field.
- Clicking a checkbox's text toggles it.
- Closing a dropdown by clicking outside used to leave its list drawn without
  a background; the application closes it with Escape first.
- A click on the date picker button, which is drawn partly outside the
  field, is redirected to it.

**When saving**, the PDF produced by PDFium is not used. An incremental update
is appended to the original file. It replaces only the data packet
(`datasets`), the embedded attachments and the saved form state. The
original template and Adobe's Reader Extensions signature stay intact, just
as when Adobe Reader saves the form.

## Known limitations

- Digital signatures and submitting the form by e-mail or web do not work.
- Non-embedded corporate fonts are replaced by Liberation Sans, so some text
  is slightly wider.
- The date picker is in English (PDFium's choice) and opens on the current
  month. Dates can also be typed.
- PDFium wraps the displayed text of some dropdowns onto two lines. The stored
  value is correct.
- Text selection does not work on dynamic XFA forms, because PDFium renders
  them without a text layer.
- Dynamic XFA forms are always shown in continuous view.
- If a PDF saved by this application is opened in Adobe Reader, Adobe uses
  its own saved form state from the last time it saved the file. The data is
  always correct.

## Usage

```bash
form-pdf-reader file.pdf
```

Shortcuts:

- **Files:** Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+P print,
  Ctrl+W close the tab.
- **View:** Ctrl+F search, Ctrl+wheel or Ctrl+/− zoom, Ctrl+0 actual size,
  Ctrl+Tab next tab.

Command-line tools (no window):

```bash
form-pdf-reader --info file.pdf
```

`--info` prints the document type and the XFA fixes applied.

```bash
form-pdf-reader --data file.pdf
```

`--data` prints the form data (XFA `datasets`).

```bash
form-pdf-reader --export-flat input.pdf output.pdf 150
```

`--export-flat` exports a flat image PDF at the given DPI. The older Spanish
names `--datos` and `--exportar-plano` still work.

## Installing a release

Each [GitHub release](../../releases) has ready-made packages for x86_64:

| Distribution | File | Install |
|---|---|---|
| Debian 12+, Ubuntu 22.04+ | `form-pdf-reader_*_amd64.deb` | `sudo apt install ./form-pdf-reader_*_amd64.deb` |
| Fedora | `form-pdf-reader-*.x86_64.rpm` | `sudo dnf install ./form-pdf-reader-*.x86_64.rpm` |
| Arch Linux, CachyOS | `form-pdf-reader-*-x86_64.pkg.tar.zst` | `sudo pacman -U form-pdf-reader-*-x86_64.pkg.tar.zst` |
| Any distribution | `form-pdf-reader-*-x86_64.tar.gz` | extract it and run `./instalar.sh` |
| Android 8+ (arm64) | `form-pdf-reader-*-android-arm64.apk` | open it on the phone and allow installing it |

The binary is built on Ubuntu 22.04 (glibc 2.35), so it also runs on newer
distributions.

## Building and installing

You need stable Rust, `curl` and the usual desktop libraries: Wayland or X11,
Mesa and fontconfig. `zenity` is used for some form dialogs.

PDFium with V8/XFA is not stored in the repository. This script downloads it
into `vendor/pdfium`, from
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries),
version chromium/8086, and checks its SHA-256:

```bash
scripts/descargar-pdfium.sh
```

Then build:

```bash
cargo build --release
```

To install it for your user in `~/.local`:

```bash
scripts/instalar-local.sh
```

Other ways to install it:

- `cd packaging && makepkg -si` builds and installs an Arch/CachyOS package,
  downloading PDFium if needed.
  Use `makepkg -si -d` if `cargo` comes from rustup rather than pacman. The
  package replaces the old `pdf-reader-editor` and `simple-pdf-reader`
  packages.
- `scripts/empaquetar.sh` creates a self-contained tar.gz in `dist/`.
- `scripts/appimage.sh` creates an AppImage if `appimagetool` is installed.

## Android app

`android/` contains a native Kotlin app that uses the same Rust core. The
bridge between Kotlin and Rust is generated with
[UniFFI](https://mozilla.github.io/uniffi-rs/) from `crates/ffi`. The app can:

- show a start screen with the recently opened documents;
- open PDFs from the system file picker or from other apps;
- scroll, and zoom with two fingers;
- fill in forms, including dynamic XFA, by tapping fields and typing on the
  on-screen keyboard;
- save, save as, view attachments and copy the text;
- follow the device's light or dark theme (or a fixed one, in Settings).

The app asks for no permissions: documents come from the system file picker
or from other apps, and the app keeps access only to the ones the user
opens.

PDFium and its V8 engine only work from the thread that initialised them, so
the app makes every call to the core from one dedicated thread
(`PdfEngine.kt`). The form's dialogs block that thread, not the interface,
while the user answers.

To build it you need the Android SDK and NDK, JDK 17, Gradle, `cargo-ndk`
and the Rust target `aarch64-linux-android`.

```bash
scripts/android/build-rust.sh
```

The script downloads PDFium for Android arm64 (checking its SHA-256),
generates the Kotlin bindings and cross-compiles the bridge. Then build the
APK:

```bash
gradle -p android assembleRelease
```

The `Android` workflow does the same on GitHub and attaches the APK to each
release. The APK is signed with the debug key so it can be installed
directly; publishing it in a store needs a real signing key.

## Tests

```bash
cargo test
```

The integration test always covers a normal PDF and an AcroForm.

The dynamic XFA part needs the blank Banco de España application form. Place
it at `diagnostico/TINTERNET_original.pdf`; it is not in the repository. That
part fills in fields, uses dropdowns and the date picker, shows sections,
works through the attachment section, saves, checks that the original stays
intact as a prefix, exports to A4 and reopens.

If a real filled-in form exists at `diagnostico/formulario_real.pdf`, the
test also checks that opening it changes no data and that the form's
"Validar" button accepts it. Missing files make those parts skip.

`PDFRE_SCRIPT_DEBUG=1`, together with `RUST_LOG=form_pdf_reader=debug`,
logs the exceptions raised by each form event.

`crates/core/examples/probe.rs` drives a form without a window (clicks, typing,
JavaScript evaluation, screenshots of a region):

```bash
cargo run --release -p form-pdf-reader-core --example probe -- file.pdf "click:561,1560;shot:1500,200,/tmp/a.png;datos"
```

A UI driver can run the real interface unattended. `PDFRE_AUTOTEST_FILE`
sets the file the file picker returns:

```bash
PDFRE_AUTOTEST="wait:30;click:300,500;type:Hello;key:Tab;shot:/tmp/a.png;quit" form-pdf-reader f.pdf
```

## Layout

The repository is a Cargo workspace. The core library is independent of the
user interface: the desktop app and the Android app both use it.

```
crates/core/      form-pdf-reader-core: the library (imported as form_pdf_reader)
  src/pdfium/       PDFium layer: bindings (bindgen), callbacks, document, fonts
  src/xfa/          XFA compatibility fixes, incremental PDF editing, form state
  src/i18n.rs       translations (catalogs in locales/)
  tests/            integration test and sample PDFs
  examples/         diagnostic tools (probe, packet dump)
crates/ffi/       form-pdf-reader-ffi: UniFFI bridge used by the Android app
crates/desktop/   form-pdf-reader: the Linux desktop application (egui)
  src/viewer.rs     view: tiles, mouse and keyboard to PDFium, tools, layouts
  src/app.rs        application: tabs, menus, dialogs, saving, printing
  src/autotest.rs   UI test driver
android/          Android app (Kotlin) on top of the Rust core
assets/           icons and .desktop file
packaging/ scripts/   packages, PDFium download, install scripts
```

## Licences

Application code: MIT. PDFium: BSD-3-Clause, with third-party licences (V8,
FreeType, ICU…) in `vendor/pdfium/licenses` after downloading it.
