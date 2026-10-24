#!/usr/bin/env bash
set -euo pipefail
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
# Isolated, disposable container store; production never sets this variable.
export XDG_CONFIG_HOME=/tmp/aidash-desktop-validation/config
export XDG_DATA_HOME=/tmp/aidash-desktop-validation/data
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" /tmp/aidash-native-browser
printf 'fixture-keyring-password' | gnome-keyring-daemon --unlock --components=secrets
cat > /tmp/aidash-native-browser/xdg-open <<'BROWSER'
#!/usr/bin/env python3
import sys, urllib.request
# Stand-in external browser for a deterministic broker fixture; no Google login.
assert sys.argv[1].startswith('http://127.0.0.1:1908')
with urllib.request.urlopen(sys.argv[1], timeout=10) as response:
    response.read()
BROWSER
chmod +x /tmp/aidash-native-browser/xdg-open
export PATH="/tmp/aidash-native-browser:$PATH"
python3 /workspace/desktop/tests/native_smoke.py
