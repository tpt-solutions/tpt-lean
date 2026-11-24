@echo off
rem SPDX-License-Identifier: MIT OR Apache-2.0
rem Build and run the C harness with MSVC (cl.exe). Verified on Windows with the Visual Studio
rem 2022 BuildTools toolset 14.44 and Windows SDK 10.0.26100.0. vcvars64.bat failed on that
rem machine, so the toolset paths are set directly; adjust them if your install differs.
rem Requires `cargo build -p tpt-lean-ffi` first (produces target\debug\tpt_lean_ffi.dll.lib).
setlocal
set "REPO=%~dp0..\..\..\.."
set "MSVC=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
set "SDK=C:\Program Files (x86)\Windows Kits\10"
set "PATH=%MSVC%\bin\Hostx64\x64;%PATH%"
set "INCLUDE=%MSVC%\include;%SDK%\Include\10.0.26100.0\ucrt;%SDK%\Include\10.0.26100.0\um;%SDK%\Include\10.0.26100.0\shared"
set "LIB=%MSVC%\lib\x64;%SDK%\Lib\10.0.26100.0\ucrt\x64;%SDK%\Lib\10.0.26100.0\um\x64"
set "OUT=%REPO%\target\ffi-c"
if not exist "%OUT%" mkdir "%OUT%"
copy /Y "%REPO%\target\debug\tpt_lean_ffi.dll" "%OUT%\" >nul || exit /b 1
copy /Y "%REPO%\target\debug\tpt_lean_ffi.dll.lib" "%OUT%\" >nul || exit /b 1
pushd "%OUT%"
cl /nologo /W4 /WX "%~dp0harness.c" /I "%REPO%\crates\tpt-lean-ffi\include" tpt_lean_ffi.dll.lib /Fe:harness.exe || (popd & exit /b 1)
.\harness.exe
set "RC=%errorlevel%"
popd
exit /b %RC%
