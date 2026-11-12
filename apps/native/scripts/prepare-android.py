"""Restrict non-HTTPS native WebView loads to the device's loopback host."""
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1] / "src-tauri/gen/android/app/src/main"
manifest = root / "AndroidManifest.xml"
if not manifest.is_file():
    raise SystemExit("Android project missing; run tauri android init first")
source = manifest.read_text(encoding="utf-8")
match = re.search(r"<application\b[^>]*>", source)
if not match:
    raise SystemExit("AndroidManifest.xml has no application tag")
tag = match.group()
if "android:networkSecurityConfig=" in tag:
    raise SystemExit("Android network policy already exists; review it manually")
updated = tag[:-1] + ' android:networkSecurityConfig="@xml/indexloom_network_security">'
manifest.write_text(source.replace(tag, updated, 1), encoding="utf-8")
directory = root / "res/xml"
directory.mkdir(parents=True, exist_ok=True)
(directory / "indexloom_network_security.xml").write_text(
    '''<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
  <base-config cleartextTrafficPermitted="false">
    <trust-anchors><certificates src="system"/></trust-anchors>
  </base-config>
  <!-- Local autonomous server is bound to 127.0.0.1, never to the LAN. -->
  <domain-config cleartextTrafficPermitted="true">
    <domain includeSubdomains="false">127.0.0.1</domain>
    <domain includeSubdomains="false">localhost</domain>
    <domain includeSubdomains="false">tauri.localhost</domain>
  </domain-config>
</network-security-config>
''',
    encoding="utf-8",
)
print("Android permits cleartext only for loopback and Tauri app hosts")
