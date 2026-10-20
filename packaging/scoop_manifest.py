"""The Scoop manifest for a lazyrunner release: `bucket/lazyrunner.json` in this repo, so

    scoop bucket add lazyrunner https://github.com/momcilovicluka/lazyrunner
    scoop install lazyrunner/lazyrunner

installs it. The release workflow (.github/workflows/publish-packages.yml) runs this after each release with the
release's two Windows archives and commits the result; `scoop update lazyrunner` then finds the new version. The
hashes and where lazyrunner.exe sits come from the archives themselves.

    python packaging/scoop_manifest.py TAG X64_ZIP ARM64_ZIP > bucket/lazyrunner.json
"""

import hashlib
import json
import posixpath
import re
import sys
import zipfile

REPO = "https://github.com/momcilovicluka/lazyrunner"
ARCHES = {"64bit": "lazyrunner-x86_64-pc-windows-msvc.zip", "arm64": "lazyrunner-aarch64-pc-windows-msvc.zip"}


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def folder_of_exe(path):
    """The folder lazyrunner.exe is in inside the archive ("" at its root)."""
    with zipfile.ZipFile(path) as archive:
        found = [name for name in archive.namelist() if posixpath.basename(name).lower() == "lazyrunner.exe"]
    if len(found) != 1:
        sys.exit(f"{path}: expected one lazyrunner.exe in it, found {len(found)}")
    return posixpath.dirname(found[0])


def manifest(tag, zips):
    version = tag[1:] if tag.startswith("v") else tag
    url = lambda arch, at: f"{REPO}/releases/download/{at}/{ARCHES[arch]}"
    architecture, autoupdate = {}, {}
    for arch, path in zips.items():
        folder = folder_of_exe(path)
        here = {"url": url(arch, tag), "hash": sha256(path)}
        later = {"url": url(arch, tag.replace(version, "$version"))}
        if folder:
            here["extract_dir"] = later["extract_dir"] = folder
        architecture[arch], autoupdate[arch] = here, later
    return {
        "version": version,
        "description": "Runs your project's apps side by side in one terminal",
        "homepage": REPO,
        # Source-available, noncommercial (LICENSE): not an open-source licence Scoop has an identifier for.
        "license": {"identifier": "Proprietary", "url": f"{REPO}/blob/main/LICENSE"},
        "architecture": architecture,
        "bin": "lazyrunner.exe",
        "checkver": "github",
        "autoupdate": {"architecture": autoupdate, "hash": {"url": "$url.sha256"}},
    }


def main(args):
    if len(args) != 3 or not re.fullmatch(r"v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?", args[0]):
        sys.exit(__doc__)
    tag, x64, arm64 = args
    print(json.dumps(manifest(tag, {"64bit": x64, "arm64": arm64}), indent=4))


if __name__ == "__main__":
    main(sys.argv[1:])
