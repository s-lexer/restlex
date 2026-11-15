# -*- mode: python ; coding: utf-8 -*-

import subprocess, pathlib, os

# Manual version — read from version.py, bump by hand on each release.
# Scheme: X.Y (1.1 → 1.2 → ...). Auto-counter retired.
exec(compile(pathlib.Path("version.py").read_text(encoding="utf-8"), "version.py", "exec"))
version = APP_VERSION  # noqa: F821 — defined by exec above
print(f"Manual-version: {version}")

# Generate PE version info for file properties
_vparts = version.split(".")
_vi = tuple(int(x) for x in _vparts[:3]) + (0,) * (4 - len(_vparts))
_ver_info = f"""\
VSVersionInfo(
  ffi=FixedFileInfo(
    filevers={_vi},
    prodvers={_vi},
    mask=0x3f, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0,0)
  ),
  kids=[
    StringFileInfo([StringTable(u'040904B0', [
      StringStruct(u'CompanyName',      u'RestLex Dev'),
      StringStruct(u'FileDescription',  u'RestLex Backup Manager'),
      StringStruct(u'FileVersion',      u'{version}'),
      StringStruct(u'InternalName',     u'restlex'),
      StringStruct(u'LegalCopyright',   u'RestLex Dev'),
      StringStruct(u'OriginalFilename', u'restlex.exe'),
      StringStruct(u'ProductName',      u'RestLex'),
      StringStruct(u'ProductVersion',   u'{version}'),
    ])]),
    VarFileInfo([VarStruct(u'Translation', [0x0409, 1200])])
  ]
)
"""
_ver_file = pathlib.Path("file_version_info.txt")
_ver_file.write_text(_ver_info, encoding="utf-8")

a = Analysis(
    ['restlex.py'],
    pathex=[],
    binaries=[],
    datas=[('i18n/en.json', 'i18n')],
    hiddenimports=['win32crypt', 'win32file', 'win32con', 'win32cryptcon', 'pywintypes'],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='restlex',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    manifest='admin.manifest',
    uac_admin=True,
    icon=['icon.ico'],
    version='file_version_info.txt',
)
