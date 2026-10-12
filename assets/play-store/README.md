# Google Play assets

- `icon-512.png`: app icon for the store (512 × 512).
- `feature-graphic.png`: feature graphic (1024 × 500).
- `phone/`: phone screenshots (1080 × 1920).
- `tablet-7/`, `tablet-10/`: tablet screenshots (1920 × 1080 and 2560 × 1440).
- `listing.md`: short and full descriptions, in English (default) and Spanish.
- `make_demo_pdfs.py`: writes the two fictitious PDFs shown in the screenshots
  (`python3 make_demo_pdfs.py <folder>`).

The screenshots were taken on a Galaxy Tab S9+ with `adb shell wm size`
(1080x1920 for the phone ones, 1440x2560 in landscape for the tablet ones),
the app in English, and the status bar replaced by a plain one with the time.
