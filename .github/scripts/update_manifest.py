import json
import sys
from datetime import datetime, timezone
from pathlib import Path

dist, version, repository, tag, notes = sys.argv[1:6]
folder = Path(dist)
base = f"https://github.com/{repository}/releases/download/{tag}"

platforms = {
    "darwin-aarch64": f"StaDesk_{version}_aarch64.app.tar.gz",
    "linux-x86_64": f"StaDesk_{version}_amd64.AppImage",
    "windows-x86_64": f"StaDesk_{version}_x64-setup.exe",
}

manifest = {
    "version": version,
    "notes": Path(notes).read_text(encoding="utf-8").strip(),
    "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {},
}
for platform, name in platforms.items():
    signature = folder / f"{name}.sig"
    if not (folder / name).is_file() or not signature.is_file():
        sys.exit(f"missing {name} or its signature")
    manifest["platforms"][platform] = {
        "signature": signature.read_text(encoding="utf-8").strip(),
        "url": f"{base}/{name}",
    }
print(json.dumps(manifest, indent=2))
