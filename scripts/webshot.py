# SPDX-FileCopyrightText: 2026 The SplatStudy contributors
# SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
"""Screenshot SplatStudy's web build in headless Edge and print its console.

    python scripts/webshot.py URL OUT.png [--size 1400x850] [--wait 6]
        [--dpr 1] [--js EXPRESSION | --cdp 'Method {json}' | --pick FILE]...
        [--downloads DIR]

Starts a separate headless Edge with a fresh profile (never the user's
browser) on a free DevTools port, so several copies can run at once. It loads
URL, waits, then runs the steps in the order given: --js evaluates an
expression in the page (promises are awaited; @FILE reads it from a file),
--cdp sends a DevTools command such as
'Input.insertText {"text": "日本"}', and --pick FILE gives a file to the
page's <input type=file> as if picked in the dialog (send
'Page.setInterceptFileChooserDialog {"enabled": true}' first so no dialog
opens). Then it takes the screenshot, prints the
page's console, exceptions and step results, and closes Edge. With
--downloads, files the page downloads are saved into DIR.

Needs the websocket-client package. Set EDGE to use another Chromium.
"""

import argparse
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

import websocket  # pip install websocket-client

EDGE = os.environ.get(
    "EDGE", r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
)


def devtools_port(profile, edge):
    """Edge writes the port it picked into the profile once it listens."""
    path = os.path.join(profile, "DevToolsActivePort")
    for _ in range(100):
        if edge.poll() is not None:
            raise SystemExit("Edge exited before DevTools was ready")
        try:
            with open(path) as f:
                return int(f.readline())
        except (OSError, ValueError):
            time.sleep(0.1)
    raise SystemExit("Edge never opened a DevTools port")


def page_socket(port):
    for _ in range(50):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json") as r:
                targets = json.load(r)
            url = next(t["webSocketDebuggerUrl"] for t in targets if t["type"] == "page")
            return websocket.create_connection(url, timeout=60)
        except (OSError, StopIteration):
            time.sleep(0.2)
    raise SystemExit("no page target")


class Page:
    def __init__(self, ws):
        self.ws = ws
        self.next_id = 0
        self.log = []

    def call(self, method, **params):
        self.next_id += 1
        mine = self.next_id
        self.ws.send(json.dumps({"id": mine, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            self.note(msg)
            if msg.get("id") == mine:
                return msg.get("result", msg.get("error"))

    def note(self, msg):
        method = msg.get("method")
        if method == "Runtime.consoleAPICalled":
            p = msg["params"]
            text = " ".join(str(a.get("value", a.get("description", ""))) for a in p["args"])
            self.log.append(f"{p['type']}: {text}")
        elif method == "Runtime.exceptionThrown":
            details = msg["params"]["exceptionDetails"]
            text = details.get("exception", {}).get("description") or details.get("text")
            self.log.append(f"exception: {text}")


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("url")
    ap.add_argument("out")
    ap.add_argument("--size", default="1400x850", help="viewport, WxH in CSS pixels")
    ap.add_argument("--wait", type=float, default=6.0, help="seconds to wait after loading")
    ap.add_argument("--dpr", type=float, default=1.0, help="device pixel ratio")
    ap.add_argument(
        "--js",
        dest="steps",
        action="append",
        default=[],
        type=lambda v: ("js", v),
        help="evaluate in the page after the wait; @FILE reads it from a file",
    )
    ap.add_argument(
        "--cdp",
        dest="steps",
        action="append",
        type=lambda v: ("cdp", v),
        help="send a DevTools command after the wait: 'Method {json params}'",
    )
    ap.add_argument(
        "--pick",
        dest="steps",
        action="append",
        type=lambda v: ("file", v),
        help="give this file to the page's <input type=file>, as if picked",
    )
    ap.add_argument("--downloads", help="save the page's downloads into this folder")
    args = ap.parse_args()
    width, height = (int(v) for v in args.size.split("x"))

    profile = tempfile.mkdtemp(prefix="webshot-")
    edge = subprocess.Popen(
        [
            EDGE,
            "--headless=new",
            "--enable-unsafe-webgpu",
            "--no-first-run",
            "--disable-sync",
            f"--user-data-dir={profile}",
            "--remote-debugging-port=0",
            "about:blank",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        page = Page(page_socket(devtools_port(profile, edge)))
        page.call("Runtime.enable")
        page.call("Page.enable")
        page.call(
            "Emulation.setDeviceMetricsOverride",
            width=width,
            height=height,
            deviceScaleFactor=args.dpr,
            mobile=False,
        )
        if args.downloads:
            folder = os.path.abspath(args.downloads)
            os.makedirs(folder, exist_ok=True)
            page.call("Browser.setDownloadBehavior", behavior="allow", downloadPath=folder)
        page.call("Page.navigate", url=args.url)
        end = time.time() + args.wait
        while time.time() < end:
            page.call("Runtime.evaluate", expression="0")  # collects console output
            time.sleep(0.25)
        for kind, step in args.steps:
            if kind == "js":
                js = step
                if js.startswith("@"):
                    with open(js[1:], encoding="utf-8") as f:
                        js = f.read()
                result = page.call(
                    "Runtime.evaluate", expression=js, awaitPromise=True, returnByValue=True
                )
                value = result.get("result", result) if isinstance(result, dict) else result
                page.log.append(f"js {js[:60]!r} -> {json.dumps(value, ensure_ascii=False)[:300]}")
            elif kind == "file":
                # Puts a file into the page's <input type=file> and fires its
                # change event, as if picked in the file dialog.
                root = page.call("DOM.getDocument")["root"]["nodeId"]
                node = page.call("DOM.querySelector", nodeId=root, selector="input[type=file]")
                result = page.call(
                    "DOM.setFileInputFiles", files=[os.path.abspath(step)], nodeId=node["nodeId"]
                )
                page.log.append(f"file {step} -> {json.dumps(result)[:300]}")
            else:
                method, _, params = step.partition(" ")
                result = page.call(method, **json.loads(params or "{}"))
                page.log.append(f"cdp {method} -> {json.dumps(result)[:300]}")
            for _ in range(4):
                page.call("Runtime.evaluate", expression="0")
                time.sleep(0.25)
        shot = page.call("Page.captureScreenshot", format="png")
        with open(args.out, "wb") as f:
            f.write(base64.b64decode(shot["data"]))
        for line in page.log:
            print(line)
        print("saved", args.out)
        page.ws.close()
    finally:
        edge.terminate()
        try:
            edge.wait(timeout=10)
        except subprocess.TimeoutExpired:
            edge.kill()
        shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    main()
