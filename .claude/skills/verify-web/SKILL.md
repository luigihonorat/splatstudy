---
name: verify-web
description: Build SplatStudy's web version, serve it locally and check it in headless Edge with a screenshot that you actually look at. Use after any change that affects what the browser build shows or does, before calling the change done.
---

<!-- SPDX-FileCopyrightText: 2026 The SplatStudy contributors -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception -->

# Verify the web build

"Done" in this project means a test passed or a screenshot was looked at.
For the browser build:

1. **Build.** `cargo xtask web`. It ends with a size list; the wasm must stay
   under the 12 MiB budget (xtask fails otherwise) and no file over 24 MiB.
2. **Serve** in the background on a port of your own (other sessions may use
   8080): `cargo xtask serve --port 81xx`. On Windows the running server
   locks `target/debug/xtask.exe`, so stop it before the next `cargo xtask`
   build that recompiles xtask, or serve from a copy of the exe.
3. **Screenshot** with the in-repo script (headless Edge, fresh profile,
   free DevTools port):

   ```
   # Python with websocket-client; in the maintainer's checkout:
   # ../tools/venv/Scripts/python.exe
   python scripts/webshot.py \
     "http://127.0.0.1:81xx/?lang=ja" <scratchpad>/ja.png --wait 5
   ```

   Then **Read the PNG** and describe what you see. Check `?lang=en` and
   `?lang=fr` too when strings or layout changed. The script prints the
   page's console: any `exception:` or `error:` line is a failure.
4. **Interact** when the change needs it. Steps run in order after the wait:
   - `--js "expr"` or `--js @file.js` evaluates in the page (promises are
     awaited, the result is printed).
   - `--cdp 'Method {json}'` sends a DevTools command, e.g.
     `--cdp 'Input.imeSetComposition {"text":"にほん","selectionStart":3,"selectionEnd":3}'`
     then `--cdp 'Input.insertText {"text":"日本"}'` to test Japanese input.
   - `--downloads DIR` saves what the page downloads (Ctrl+S on a scene, an
     OBJ export).
   - `--pick FILE` answers the app's file picker: send
     `--cdp 'Page.setInterceptFileChooserDialog {"enabled":true}'` first, then
     the key or menu that opens it (`Input.dispatchKeyEvent` with
     `"modifiers":2` for Ctrl+O), then `--pick`. A saved scene re-opened this
     way is the save/open round trip.

## Tips

- **Clicks:** CDP `Input.dispatchMouseEvent` clicks don't register as egui
  clicks here. Dispatch `PointerEvent`s on `#splatstudy` from `--js`
  instead, with two `requestAnimationFrame` waits between `pointermove`,
  `pointerdown` and `pointerup`.
- **Files:** to test a drop, build a `File` in JS, put it in a
  `DataTransfer` and dispatch a `drop` `DragEvent` on the canvas. Generate
  tiny fixtures in the script (an OBJ octahedron is 14 lines); never copy
  files from elsewhere.
- **URL options:** `?sample=<id>` loads a bundled sample, `?debug` shows the
  pen HUD. `?view=`, `?sculpt=`, `?reference=`, `?grid=`, `?hide=`, `?tool=`,
  `?symmetry`, `?timer=` set the viewpoints, aids, panels and tool at start
  (all listed in `crates/core/src/app/start.rs`); with any of them the page
  opens a scratch session, without restoring or writing the autosave.
- Notices last 4 s after the frame that posts them; take the screenshot
  soon after the action.
