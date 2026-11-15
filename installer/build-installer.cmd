@echo off
REM Builds RestLex-Setup.exe with Inno Setup and signs it via the EV sign-service.
setlocal
set ISCC="C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
set SCRIPT_DIR=%~dp0

%ISCC% "%SCRIPT_DIR%RestLex-Setup.iss"
if errorlevel 1 (
    echo Inno Setup compile failed.
    exit /b 1
)

python "%SCRIPT_DIR%..\_sign.py" "%SCRIPT_DIR%..\dist\RestLex-Setup.exe"
if errorlevel 1 (
    echo Signing failed.
    exit /b 1
)

echo Done: dist\RestLex-Setup.exe
endlocal
