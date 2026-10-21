# UI verification tools

Scripts used to verify the GUI without a human looking at it. They are not part of
the build; they exist so a UI claim can be backed by evidence.

## Why these exist

An agent cannot see a screenshot. Without these, "the UI renders correctly" would be
an assumption. With them, every layout claim is backed by either OCR text extracted
from the window or a pixel histogram.

## Tools

| script | what it proves |
|---|---|
| `shot.ps1` | Captures a window's pixels via `PrintWindow`, enumerating windows by PID rather than trusting `MainWindowHandle` (which is often a 14×14 hidden window). |
| `ocr.ps1` | Extracts the visible text with the Windows OCR engine, so layout and copy can be checked. |
| `pixstat.ps1` | Colour histogram of a capture: proves the palette actually applied (dark vs light, accent present, no blank frame). |
| `set_ui.py` | Sets fields in `ui.toml` **with correct TOML quoting**. An earlier version wrote `theme = light`, which is invalid TOML; the app then fell back to defaults and looked like it was ignoring the theme. That was a false bug report caused by the test tool. |

## Usage

```powershell
# capture + read a window
powershell -NoProfile -File tools/ui/shot.ps1 -Exe .\target\debug\omenmon-gui.exe -OutFile ui.png
powershell -NoProfile -File tools/ui/ocr.ps1 -Path ui.png
powershell -NoProfile -File tools/ui/pixstat.ps1 -Path ui.png

# drive the persisted UI state
python tools/set_ui.py "theme=light" "page=performance"
```

## Known limitation

`PrintWindow` returns a **blank frame** for the GUI when it runs elevated: the
window is GPU-composited on a different desktop context. Pixel capture therefore
only works for the unelevated window. Elevated state is verified through the data
path instead (`omenmon power`, `temps`, `kbd`, `bios`), which is what the cards
actually read.
