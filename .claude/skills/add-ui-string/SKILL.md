---
name: add-ui-string
description: Add or change a UI string in SplatStudy in Japanese, English and French, in the right per-part string table, then make sure the embedded Japanese font covers it. Use whenever UI text is added or reworded.
---

<!-- SPDX-FileCopyrightText: 2026 The SplatStudy contributors -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception -->

# Add a UI string

Every string the UI shows exists in Japanese (the primary language), English
and French. A missing language is a compile error, and a missing glyph fails
a test.

1. **Pick the table.** Each part of the editor has its own file, so parts
   built in parallel don't edit the same one:
   `crates/core/src/ui/i18n/{refs,study,mesh,files,export,help}.rs`.
   `crates/core/src/ui/i18n.rs` holds the shared strings and is frozen;
   change it only in a small commit titled `Hook: …`. A new table also needs
   a `Hook:` commit: `pub mod <part>;` and a field in `Tr` in `i18n.rs`, and
   its file in `TABLES` in `crates/core/src/ui/fonts.rs`.
2. **Add the field** to the table's struct with a short doc comment when its
   use isn't obvious, then a value in each of `JA`, `EN` and `FR`.
   - English and French are lowercase, like the site (luigihonorat.com):
     "import reference…", "premiers pas".
   - Japanese uses the app's words: モチーフ (reference), 塑像 (sculpt).
     Full-width punctuation: `：`, `（ ）`, `、`.
   - French puts a space before `:` and `?`.
3. **Use it** as `self.lang.tr().<part>.<field>`; never a literal in code.
4. **Check the font.**
   `cargo test -p splatstudy-core the_embedded_fonts_cover_every_ui_string`.
   If it lists missing characters, rebuild the subset (deterministic):

   ```
   cd crates/core/assets/fonts
   F:/dev/splatstudy/tools/venv/Scripts/python.exe subset.py F:/dev/splatstudy/fonts-src
   ```

   and run the test again. Commit `NotoSansJP-UI.ttf` with the strings. On a
   merge conflict in the font, take the other side's file and rerun the script.
5. **Look at it** in all three languages (the `verify-web` skill, `?lang=ja|en|fr`):
   long French lines and Japanese line breaks are the usual surprises.
