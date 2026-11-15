# -*- mode: python ; coding: utf-8 -*-

import subprocess, pathlib, os

# auto-version from git
count = subprocess.run(["git", "rev-list", "--count", "HEAD"], capture_output=True, text=True, cwd=".").stdout.strip()
version = f"1.0.{count or '0'}"
print(f"Installer auto-version: {version}")

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
      StringStruct(u'FileDescription',  u'RestLex Installer'),
      StringStruct(u'FileVersion',      u'{version}'),
      StringStruct(u'InternalName',     u'restlex-installer'),
      StringStruct(u'LegalCopyright',   u'RestLex Dev'),
      StringStruct(u'OriginalFilename', u'restlex-installer.exe'),
      StringStruct(u'ProductName',      u'RestLex'),
      StringStruct(u'ProductVersion',   u'{version}'),
    ])]),
    VarFileInfo([VarStruct(u'Translation', [0x0409, 1200])])
  ]
)
"""
_ver_file = pathlib.Path("installer_version_info.txt")
_ver_file.write_text(_ver_info, encoding="utf-8")

a = Analysis(
    ['restlex-installer.py'],
    pathex=[],
    binaries=[],
    datas=[],
    hiddenimports=[
        'requests',
        'urllib3',
        'certifi',
        'charset_normalizer',
        'idna',
        'win32com',
        'win32com.client',
        'pythoncom',
    ],
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
    name='restlex-installer',
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
    version='installer_version_info.txt',
)
