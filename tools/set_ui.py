"""Set fields in the GUI's ui.toml, preserving TOML types.

An earlier version of this helper wrote `theme = light` without quotes. That is not
valid TOML, so `UiState::load()` fell back to defaults and the app looked like it was
ignoring the theme - a false bug report caused by the test tool, not the app. String
fields are therefore quoted explicitly.

Usage:
    python tools/set_ui.py "theme=light" "page=performance" "compact=true"
"""
import pathlib
import re
import sys

STRING_FIELDS = {'theme', 'page'}
PATH = pathlib.Path.home() / '.config' / 'omenmon' / 'ui.toml'


def main(pairs):
    text = PATH.read_text(encoding='utf-8')
    for pair in pairs:
        key, value = pair.split('=', 1)
        key, value = key.strip(), value.strip()
        if key in STRING_FIELDS:
            value = value.strip('"')
            line = f'{key} = "{value}"'
        else:
            line = f'{key} = {value}'
        text, n = re.subn(rf'(?m)^{re.escape(key)} = .*$', line, text)
        if n == 0:
            text = text.rstrip('\n') + '\n' + line + '\n'
    PATH.write_text(text, encoding='utf-8', newline='\n')
    print(text)


if __name__ == '__main__':
    main(sys.argv[1:])
